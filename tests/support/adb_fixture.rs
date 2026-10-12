use std::{io::{self, Write}, time::Duration};
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["devices", "-l"] {
        println!("List of devices attached\nemulator-5554 device product:sdk model:Pixel_8\nusb-1 unauthorized"); return;
    }
    assert_eq!(args.len(), 8);
    assert_eq!(&args[2..], &["logcat", "-v", "brief", "-T", "1", "*:V"]);
    assert_eq!(args[0], "-s");
    if args[1] == "missing" { eprintln!("device disconnected"); std::process::exit(1); }
    let mut out = io::stdout().lock();
    for index in 0.. {
        writeln!(out, "I/Fixture( 123): línea α {index}").unwrap();
        out.flush().unwrap();
        if args[1] != "flood" { std::thread::sleep(Duration::from_millis(25)); }
    }
}
