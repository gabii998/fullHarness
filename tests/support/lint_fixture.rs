fn main(){
 if std::env::args().any(|a|a=="wait"){std::thread::sleep(std::time::Duration::from_secs(30));return;}
 println!("src/main.rs:2:7: error: bad call");eprintln!("src/lib.rs:4:2: warning: unused");std::process::exit(1);
}
