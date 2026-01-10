use kube::CustomResourceExt;
use kulta::crd::rollout::Rollout as RolloutV1alpha1;
use kulta::crd::v1beta1::Rollout as RolloutV1beta1;
use serde_json::{json, Value};

fn main() -> anyhow::Result<()> {
    // Generate CRD with both versions and conversion webhook
    // Use: cargo run --bin gen-crd | python3 -c "import sys,json,yaml; print(yaml.dump(json.load(sys.stdin), default_flow_style=False))"
    // to convert to YAML

    // Get base CRD from v1alpha1 (storage version)
    let mut crd: Value = serde_json::to_value(RolloutV1alpha1::crd())?;

    // Get v1beta1 version schema
    let v1beta1_crd: Value = serde_json::to_value(RolloutV1beta1::crd())?;

    // Extract v1beta1 version entry
    let v1beta1_version = v1beta1_crd["spec"]["versions"][0].clone();

    // Mark v1alpha1 as NOT the storage version (v1beta1 will be storage)
    if let Some(versions) = crd["spec"]["versions"].as_array_mut() {
        if let Some(v1alpha1) = versions.get_mut(0) {
            v1alpha1["storage"] = json!(false);
            v1alpha1["served"] = json!(true);
        }
        // Add v1beta1 as the storage version
        let mut v1beta1 = v1beta1_version.clone();
        v1beta1["storage"] = json!(true);
        v1beta1["served"] = json!(true);
        versions.push(v1beta1);
    }

    // Add conversion webhook configuration
    crd["spec"]["conversion"] = json!({
        "strategy": "Webhook",
        "webhook": {
            "clientConfig": {
                "service": {
                    "name": "kulta-controller",
                    "namespace": "kulta-system",
                    "path": "/convert",
                    "port": 8443
                }
            },
            "conversionReviewVersions": ["v1"]
        }
    });

    let json_output = serde_json::to_string_pretty(&crd)?;
    println!("{}", json_output);
    Ok(())
}
