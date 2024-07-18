use std::net::UdpSocket;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Public server's IP
    #[arg(short, long)]
    port: String,

    #[arg(short, long, default_value_t=String::from("hello"))]
    response: String,
}

fn main() {
    let args = Args::parse();
    let udp = UdpSocket::bind(format!("0.0.0.0:{}", args.port)).unwrap();
    let mut buf = [0u8; u16::MAX as usize];
    while let Ok((size, from)) = udp.recv_from(&mut buf) {
        let s = format!(
            "{} {}",
            args.response,
            String::from_utf8_lossy(&buf[..size])
        );
        udp.send_to(s.as_bytes(), from).unwrap();
    }
}
