use std::net::UdpSocket;

fn main() {
	let udp =  UdpSocket::bind("0.0.0.0:7080").unwrap();
	udp.connect("127.0.0.1:6600").unwrap();
	udp.send(b"me").unwrap();
	let mut buf = [0u8;u16::MAX as usize];
	let size = udp.recv(& mut buf).unwrap();
	println!("{}",String::from_utf8_lossy(&buf[..size]));
}
