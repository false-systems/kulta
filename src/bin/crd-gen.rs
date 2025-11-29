use kulta::crd::rollout::Rollout;
use kube::CustomResourceExt;

fn main() {
    print!("{}", serde_json::to_string_pretty(&Rollout::crd()).unwrap());
}
