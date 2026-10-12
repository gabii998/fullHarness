// Native offline Docker fixture: never accesses the daemon or a network.
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::env::current_dir().unwrap();
    let owned = "a".repeat(64); let other = "b".repeat(64);
    if args[0] == "ps" {
        assert!(args.iter().any(|s| s == "label=com.docker.compose.project.working_dir"));
        println!("{owned}\n{other}");
    } else if args[0] == "container" && args[1] == "inspect" {
        assert!(!args[3].contains(".Config.Env"));
        for id in &args[4..] {
            let directory = if id == &other || root.join("moved").exists() { root.join("other") } else { root.clone() };
            println!("{{\"id\":{},\"name\":\"/db\",\"image\":\"postgres:17\",\"state\":\"running\",\"health\":\"healthy\",\"service\":\"db\",\"project\":\"same-name\",\"directory\":{},\"ports\":{{\"5432/tcp\":[{{\"HostPort\":\"5432\"}}]}}}}",quote(id),quote(&directory.to_string_lossy()));
        }
    } else if args[0] == "container" {
        assert_eq!(args.last().unwrap(), &owned);
        if args[1] != "start" { assert_eq!(&args[2..4], ["--time", "10"]); }
        std::fs::write(root.join("mutated"), &args[1]).unwrap();
        println!("{owned}");
    } else if args[0] == "logs" {
        assert_eq!(&args[..4], ["logs", "--tail", "200", "--timestamps"]);
        println!("stdout log"); eprintln!("stderr log");
    } else if args[0] == "compose" {
        assert_eq!(args[1], "--project-directory"); assert_eq!(args[3], "-f");
        assert_eq!(&args[args.len()-2..], ["up", "-d"]);
        if root.join("compose.override.yaml").exists() { assert_eq!(args[5], "-f"); assert!(args[6].ends_with("compose.override.yaml")); }
        assert!(std::path::Path::new(&args[4]).canonicalize().unwrap().starts_with(root.canonicalize().unwrap()));
        assert!(std::env::var_os("COMPOSE_FILE").is_none());
        std::fs::write(root.join("compose-up"), args.join("\n")).unwrap();
    } else { panic!("unexpected arguments {args:?}"); }
}
