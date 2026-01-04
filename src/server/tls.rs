//! TLS certificate generation for webhook HTTPS
//!
//! Generates self-signed certificates for the conversion webhook.
//! The controller creates a CA and server certificate on startup,
//! then patches the CRD with the CA bundle.
//!
//! ## Certificate Chain
//! ```text
//! Self-signed CA (kulta-webhook-ca)
//!     └── Server cert (kulta-controller.kulta-system.svc)
//! ```

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType,
};
use std::sync::Arc;
use thiserror::Error;

/// Default validity period for CA certificate (10 years)
pub const CA_VALIDITY_DAYS: u32 = 3650;

/// Default validity period for server certificate (1 year)
pub const SERVER_VALIDITY_DAYS: u32 = 365;

/// Errors that can occur during TLS setup
#[derive(Debug, Error)]
pub enum TlsError {
    #[error("Failed to generate key pair: {0}")]
    KeyGeneration(#[from] rcgen::Error),

    #[error("Failed to serialize certificate: {0}")]
    Serialization(String),

    #[error("Failed to parse certificate: {0}")]
    Parse(String),

    #[error("Kubernetes API error: {0}")]
    Kube(#[from] kube::Error),

    #[error("Invalid PEM data")]
    InvalidPem,
}

/// Generated certificate bundle containing CA and server certificates
#[derive(Clone)]
pub struct CertificateBundle {
    /// PEM-encoded CA certificate
    pub ca_cert_pem: String,
    /// PEM-encoded server certificate
    pub server_cert_pem: String,
    /// PEM-encoded server private key
    pub server_key_pem: String,
}

impl CertificateBundle {
    /// Get the CA certificate as base64-encoded DER (for caBundle in CRD)
    pub fn ca_bundle_base64(&self) -> Result<String, TlsError> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        // Extract DER from PEM
        let pem = pem::parse(&self.ca_cert_pem)
            .map_err(|e| TlsError::Parse(format!("Failed to parse CA PEM: {}", e)))?;

        Ok(STANDARD.encode(pem.contents()))
    }
}

/// Generate a self-signed CA certificate
fn generate_ca() -> Result<(Certificate, KeyPair), TlsError> {
    let mut params = CertificateParams::default();

    params
        .distinguished_name
        .push(DnType::CommonName, "kulta-webhook-ca");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "kulta");

    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    // Set validity
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(CA_VALIDITY_DAYS as i64);

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    Ok((cert, key_pair))
}

/// Generate a server certificate signed by the CA
fn generate_server_cert(
    ca_cert: &Certificate,
    ca_key: &KeyPair,
    service_name: &str,
    namespace: &str,
) -> Result<(String, String), TlsError> {
    let mut params = CertificateParams::default();

    params
        .distinguished_name
        .push(DnType::CommonName, service_name);
    params
        .distinguished_name
        .push(DnType::OrganizationName, "kulta");

    // DNS names for the service
    params.subject_alt_names = vec![
        SanType::DnsName(
            service_name
                .try_into()
                .map_err(|e| TlsError::Serialization(format!("Invalid service name: {}", e)))?,
        ),
        SanType::DnsName(
            format!("{}.{}", service_name, namespace)
                .try_into()
                .map_err(|e| TlsError::Serialization(format!("Invalid DNS name: {}", e)))?,
        ),
        SanType::DnsName(
            format!("{}.{}.svc", service_name, namespace)
                .try_into()
                .map_err(|e| TlsError::Serialization(format!("Invalid DNS name: {}", e)))?,
        ),
        SanType::DnsName(
            format!("{}.{}.svc.cluster.local", service_name, namespace)
                .try_into()
                .map_err(|e| TlsError::Serialization(format!("Invalid DNS name: {}", e)))?,
        ),
    ];

    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    // Set validity
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(SERVER_VALIDITY_DAYS as i64);

    let key_pair = KeyPair::generate()?;
    let cert = params.signed_by(&key_pair, ca_cert, ca_key)?;

    Ok((cert.pem(), key_pair.serialize_pem()))
}

/// Generate a complete certificate bundle for the webhook
///
/// Creates:
/// - Self-signed CA certificate
/// - Server certificate signed by the CA
///
/// # Arguments
/// * `service_name` - Kubernetes service name (e.g., "kulta-controller")
/// * `namespace` - Kubernetes namespace (e.g., "kulta-system")
pub fn generate_certificate_bundle(
    service_name: &str,
    namespace: &str,
) -> Result<CertificateBundle, TlsError> {
    // Generate CA
    let (ca_cert, ca_key) = generate_ca()?;
    let ca_cert_pem = ca_cert.pem();

    // Generate server cert
    let (server_cert_pem, server_key_pem) =
        generate_server_cert(&ca_cert, &ca_key, service_name, namespace)?;

    Ok(CertificateBundle {
        ca_cert_pem,
        server_cert_pem,
        server_key_pem,
    })
}

/// Secret keys for storing certificate data
pub const SECRET_CA_CERT_KEY: &str = "ca.crt";
pub const SECRET_SERVER_CERT_KEY: &str = "tls.crt";
pub const SECRET_SERVER_KEY_KEY: &str = "tls.key";

/// Default secret name for webhook TLS
pub const DEFAULT_TLS_SECRET_NAME: &str = "kulta-webhook-tls";

/// Load certificate bundle from a Kubernetes Secret
pub async fn load_from_secret(
    client: &kube::Client,
    namespace: &str,
    secret_name: &str,
) -> Result<Option<CertificateBundle>, TlsError> {
    use k8s_openapi::api::core::v1::Secret;
    use kube::Api;

    let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);

    match secrets.get(secret_name).await {
        Ok(secret) => {
            let data = secret.data.unwrap_or_default();

            let ca_cert_pem = data
                .get(SECRET_CA_CERT_KEY)
                .map(|b| String::from_utf8_lossy(&b.0).to_string())
                .ok_or_else(|| TlsError::Parse("Missing ca.crt in secret".to_string()))?;

            let server_cert_pem = data
                .get(SECRET_SERVER_CERT_KEY)
                .map(|b| String::from_utf8_lossy(&b.0).to_string())
                .ok_or_else(|| TlsError::Parse("Missing tls.crt in secret".to_string()))?;

            let server_key_pem = data
                .get(SECRET_SERVER_KEY_KEY)
                .map(|b| String::from_utf8_lossy(&b.0).to_string())
                .ok_or_else(|| TlsError::Parse("Missing tls.key in secret".to_string()))?;

            Ok(Some(CertificateBundle {
                ca_cert_pem,
                server_cert_pem,
                server_key_pem,
            }))
        }
        Err(kube::Error::Api(err)) if err.code == 404 => Ok(None),
        Err(e) => Err(TlsError::Kube(e)),
    }
}

/// Save certificate bundle to a Kubernetes Secret
pub async fn save_to_secret(
    client: &kube::Client,
    namespace: &str,
    secret_name: &str,
    bundle: &CertificateBundle,
) -> Result<(), TlsError> {
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use k8s_openapi::ByteString;
    use kube::api::{Patch, PatchParams, PostParams};
    use kube::Api;
    use std::collections::BTreeMap;

    let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);

    let mut data = BTreeMap::new();
    data.insert(
        SECRET_CA_CERT_KEY.to_string(),
        ByteString(bundle.ca_cert_pem.as_bytes().to_vec()),
    );
    data.insert(
        SECRET_SERVER_CERT_KEY.to_string(),
        ByteString(bundle.server_cert_pem.as_bytes().to_vec()),
    );
    data.insert(
        SECRET_SERVER_KEY_KEY.to_string(),
        ByteString(bundle.server_key_pem.as_bytes().to_vec()),
    );

    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(secret_name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some({
                let mut labels = BTreeMap::new();
                labels.insert(
                    "app.kubernetes.io/managed-by".to_string(),
                    "kulta".to_string(),
                );
                labels
            }),
            ..Default::default()
        },
        type_: Some("kubernetes.io/tls".to_string()),
        data: Some(data),
        ..Default::default()
    };

    // Try to create, if exists, patch it
    match secrets.create(&PostParams::default(), &secret).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(err)) if err.code == 409 => {
            // Already exists, patch it
            secrets
                .patch(secret_name, &PatchParams::default(), &Patch::Merge(&secret))
                .await?;
            Ok(())
        }
        Err(e) => Err(TlsError::Kube(e)),
    }
}

/// Patch the CRD with the CA bundle for webhook conversion
pub async fn patch_crd_ca_bundle(
    client: &kube::Client,
    ca_bundle_base64: &str,
) -> Result<(), TlsError> {
    use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
    use kube::api::{Patch, PatchParams};
    use kube::Api;

    let crds: Api<CustomResourceDefinition> = Api::all(client.clone());

    let patch = serde_json::json!({
        "spec": {
            "conversion": {
                "webhook": {
                    "clientConfig": {
                        "caBundle": ca_bundle_base64
                    }
                }
            }
        }
    });

    crds.patch(
        "rollouts.kulta.io",
        &PatchParams::default(),
        &Patch::Merge(&patch),
    )
    .await?;

    Ok(())
}

/// Initialize TLS certificates for the webhook
///
/// This function:
/// 1. Tries to load existing certs from a Secret
/// 2. If not found, generates new certs
/// 3. Saves the certs to a Secret
/// 4. Patches the CRD with the CA bundle
///
/// Returns the certificate bundle for use by the HTTPS server.
pub async fn initialize_tls(
    client: &kube::Client,
    service_name: &str,
    namespace: &str,
    secret_name: &str,
) -> Result<CertificateBundle, TlsError> {
    use tracing::{info, warn};

    // Try to load existing certs
    match load_from_secret(client, namespace, secret_name).await? {
        Some(bundle) => {
            info!(
                secret = secret_name,
                "Loaded existing TLS certificates from Secret"
            );

            // Still patch the CRD in case it was recreated
            let ca_bundle = bundle.ca_bundle_base64()?;
            if let Err(e) = patch_crd_ca_bundle(client, &ca_bundle).await {
                warn!(error = ?e, "Failed to patch CRD with CA bundle (may not exist yet)");
            }

            Ok(bundle)
        }
        None => {
            info!("No existing TLS certificates found, generating new ones");

            // Generate new certs
            let bundle = generate_certificate_bundle(service_name, namespace)?;

            // Save to Secret
            save_to_secret(client, namespace, secret_name, &bundle).await?;
            info!(secret = secret_name, "Saved new TLS certificates to Secret");

            // Patch the CRD
            let ca_bundle = bundle.ca_bundle_base64()?;
            if let Err(e) = patch_crd_ca_bundle(client, &ca_bundle).await {
                warn!(error = ?e, "Failed to patch CRD with CA bundle (may not exist yet)");
            }

            Ok(bundle)
        }
    }
}

/// Build a rustls ServerConfig from the certificate bundle
pub fn build_rustls_config(
    bundle: &CertificateBundle,
) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    use rustls::pki_types::CertificateDer;
    use rustls_pemfile::{certs, private_key};
    use std::io::BufReader;

    // Parse server certificate chain
    let cert_chain: Vec<CertificateDer<'static>> =
        certs(&mut BufReader::new(bundle.server_cert_pem.as_bytes()))
            .filter_map(|r| r.ok())
            .collect();

    if cert_chain.is_empty() {
        return Err(TlsError::InvalidPem);
    }

    // Parse private key
    let key = private_key(&mut BufReader::new(bundle.server_key_pem.as_bytes()))
        .map_err(|e| TlsError::Parse(format!("Failed to parse private key: {}", e)))?
        .ok_or(TlsError::InvalidPem)?;

    // Build rustls config with ring crypto provider
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| TlsError::Parse(format!("Failed to set protocol versions: {}", e)))?
    .with_no_client_auth()
    .with_single_cert(cert_chain, key)
    .map_err(|e| TlsError::Parse(format!("Failed to build TLS config: {}", e)))?;

    Ok(Arc::new(config))
}
