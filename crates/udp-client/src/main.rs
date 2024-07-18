use std::net::UdpSocket;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Public server's IP
    #[arg(short, long)]
    server: String,

    #[arg(short, long, default_value_t=String::from("me"))]
    text: String,
}

fn main() {
    let args = Args::parse();
    let udp = UdpSocket::bind("0.0.0.0:0").unwrap();
    udp.connect(format!("{}", args.server)).unwrap();
    udp.send(args.text.as_bytes()).unwrap();
    let mut buf = [0u8; u16::MAX as usize];
    let size = udp.recv(&mut buf).unwrap();
    println!(
        "receive from server, content: {}",
        String::from_utf8_lossy(&buf[..size])
    );
}
