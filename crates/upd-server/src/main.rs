use std::{fmt::format, net::UdpSocket};

fn main() {
	let udp = UdpSocket::bind("0.0.0.0:8080").unwrap();
	let mut buf = [0u8;u16::MAX as usize];
	while let Ok((size, from)) = udp.recv_from(& mut buf){
		let s = format!("hello {}",String::from_utf8_lossy(&buf[..size]));
		udp.send_to(s.as_bytes(), from).unwrap();
	}
}
