//! Tests for TLS certificate generation
//!
//! TDD: Tests written to verify certificate generation

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::tls::*;

/// Test: Certificate bundle is generated successfully
#[test]
fn test_generate_certificate_bundle_succeeds() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system");
    assert!(bundle.is_ok(), "Should generate certificate bundle");
}

/// Test: CA certificate is valid PEM
#[test]
fn test_ca_cert_is_valid_pem() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    assert!(bundle.ca_cert_pem.contains("-----BEGIN CERTIFICATE-----"));
    assert!(bundle.ca_cert_pem.contains("-----END CERTIFICATE-----"));
}

/// Test: Server certificate is valid PEM
#[test]
fn test_server_cert_is_valid_pem() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    assert!(bundle
        .server_cert_pem
        .contains("-----BEGIN CERTIFICATE-----"));
    assert!(bundle.server_cert_pem.contains("-----END CERTIFICATE-----"));
}

/// Test: Server key is valid PEM
#[test]
fn test_server_key_is_valid_pem() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    assert!(bundle
        .server_key_pem
        .contains("-----BEGIN PRIVATE KEY-----"));
    assert!(bundle.server_key_pem.contains("-----END PRIVATE KEY-----"));
}

/// Test: CA bundle can be encoded as base64
#[test]
fn test_ca_bundle_base64_encoding() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    let base64 = bundle.ca_bundle_base64();
    assert!(base64.is_ok(), "Should encode CA as base64");

    let encoded = base64.unwrap();
    assert!(!encoded.is_empty(), "Base64 should not be empty");
    // Base64 should not contain PEM markers
    assert!(!encoded.contains("-----BEGIN"));
}

/// Test: Each call generates unique certificates
#[test]
fn test_generates_unique_certs() {
    let bundle1 = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();
    let bundle2 = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    // Private keys should be different
    assert_ne!(
        bundle1.server_key_pem, bundle2.server_key_pem,
        "Each call should generate unique keys"
    );
}

/// Test: Server cert contains correct DNS names
#[test]
fn test_server_cert_contains_dns_names() {
    let bundle = generate_certificate_bundle("my-service", "my-namespace").unwrap();

    // Parse the certificate to verify DNS names
    let pem = pem::parse(&bundle.server_cert_pem).unwrap();
    let (_, cert) = x509_parser::parse_x509_certificate(pem.contents()).unwrap();

    // Get Subject Alternative Names
    let san = cert
        .subject_alternative_name()
        .expect("Should have SAN extension")
        .expect("SAN should be present");

    let dns_names: Vec<String> = san
        .value
        .general_names
        .iter()
        .filter_map(|name| {
            if let x509_parser::prelude::GeneralName::DNSName(dns) = name {
                Some(dns.to_string())
            } else {
                None
            }
        })
        .collect();

    assert!(dns_names.contains(&"my-service".to_string()));
    assert!(dns_names.contains(&"my-service.my-namespace".to_string()));
    assert!(dns_names.contains(&"my-service.my-namespace.svc".to_string()));
    assert!(dns_names.contains(&"my-service.my-namespace.svc.cluster.local".to_string()));
}

/// Test: rustls config can be built from bundle
#[test]
fn test_build_rustls_config_succeeds() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    let config = build_rustls_config(&bundle);
    assert!(
        config.is_ok(),
        "Should build rustls config: {:?}",
        config.err()
    );
}

/// Test: CA certificate has CA flag set
#[test]
fn test_ca_cert_has_ca_flag() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    let pem = pem::parse(&bundle.ca_cert_pem).unwrap();
    let (_, cert) = x509_parser::parse_x509_certificate(pem.contents()).unwrap();

    let basic_constraints = cert
        .basic_constraints()
        .expect("Should have basic constraints")
        .expect("Basic constraints should be present");

    assert!(basic_constraints.value.ca, "CA flag should be set");
}

/// Test: Server certificate is NOT a CA
#[test]
fn test_server_cert_is_not_ca() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    let pem = pem::parse(&bundle.server_cert_pem).unwrap();
    let (_, cert) = x509_parser::parse_x509_certificate(pem.contents()).unwrap();

    // Server cert may or may not have basic constraints, but if it does, ca should be false
    if let Ok(Some(basic_constraints)) = cert.basic_constraints() {
        assert!(
            !basic_constraints.value.ca,
            "Server cert should not be a CA"
        );
    }
    // If no basic constraints, that's also fine for a leaf cert
}

/// Test: Server certificate has server auth extended key usage
#[test]
fn test_server_cert_has_server_auth_eku() {
    let bundle = generate_certificate_bundle("kulta-controller", "kulta-system").unwrap();

    let pem = pem::parse(&bundle.server_cert_pem).unwrap();
    let (_, cert) = x509_parser::parse_x509_certificate(pem.contents()).unwrap();

    let eku = cert
        .extended_key_usage()
        .expect("Should have EKU extension")
        .expect("EKU should be present");

    assert!(eku.value.server_auth, "Should have server auth EKU");
}
