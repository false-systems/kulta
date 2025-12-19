use kube::CustomResourceExt;
use kulta::crd::rollout::Rollout;

fn main() -> anyhow::Result<()> {
    // Generate CRD and print as JSON
    // Use: cargo run --bin gen-crd | python3 -c "import sys,json,yaml; print(yaml.dump(json.load(sys.stdin), default_flow_style=False))"
    // to convert to YAML
    let crd = Rollout::crd();
    let json = serde_json::to_string_pretty(&crd)?;
    println!("{}", json);
    Ok(())
}
