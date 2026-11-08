//! Read-only production verifier acceptance. Run only with an explicit isolated home.
use keyzapper_core::{hermes::{cli::{Capability, HermesCli}, controller::{Controller, Inputs}, store::HermesStore, ConfigAccess, HermesTarget}, managed::ManagedConfig};
use std::path::PathBuf;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 { std::process::exit(2); }
    let target = HermesTarget { executable: PathBuf::from(&args[0]), home: PathBuf::from(&args[1]), display_name: "isolated diagnostic".into() };
    let cli = HermesCli::new();
    let supported = cli.capability(&target) == Capability::Supported;
    if !supported {
        println!("{}", serde_json::json!({"supported": false}));
        std::process::exit(1);
    }
    let before = target.home.join("config.yaml").exists();
    let path_ok = cli.snapshot(&target, &[]).is_ok_and(|snapshot| snapshot.config_path == target.home.join("config.yaml"));
    let controller = Controller::new(HermesStore::new(std::env::temp_dir().join(format!("verifier-controller-{}", uuid::Uuid::new_v4()))));
    let inputs = Inputs { profiles: vec![], policy: ManagedConfig::default(), disabled: false, helper: None, targets: vec![target.clone()], diagnostics: vec![] };
    let refreshed = controller.begin().unwrap().refresh(&cli, &inputs, |_| {}).is_ok();
    let view = controller.view();
    let published = refreshed && view.targets.len() == 1 && view.targets[0].supported && view.selected.is_none();
    let config_unchanged = before == target.home.join("config.yaml").exists();
    println!("{}", serde_json::json!({"supported": supported, "controllerPublished": published, "configPathBound": path_ok, "configExistenceUnchanged": config_unchanged}));
    if !published || !path_ok || !config_unchanged { std::process::exit(1); }
}
