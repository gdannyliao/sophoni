use qrcode::{QrCode, render::svg::Color};

/// 生成配对二维码的内容字符串：sophoni://pair?ip=&port=&code=
pub fn build_pair_url(ip: &str, port: u16, code: &str) -> String {
    format!("sophoni://pair?ip={ip}&port={port}&code={code}")
}

/// 生成二维码 SVG 字符串（前端 <img src="data:image/svg+xml,..."> 直接展示）。
pub fn render_qr_svg(content: &str) -> String {
    let code = QrCode::new(content.as_bytes()).expect("二维码内容过长");
    code.render::<Color>().min_dimensions(200, 200).build()
}

/// 取本机局域网 IPv4（第一个非 loopback 的 IPv4）。
/// 用 UDP socket 连接公网地址探测出口 IP（不实际发包）。
pub fn local_ip() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    match addr.ip() {
        std::net::IpAddr::V4(v4) => Some(v4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_url_format() {
        let url = build_pair_url("192.168.1.5", 43210, "482910");
        assert_eq!(url, "sophoni://pair?ip=192.168.1.5&port=43210&code=482910");
    }

    #[test]
    fn qr_svg_nonempty_and_valid() {
        let svg = render_qr_svg("sophoni://pair?ip=1.2.3.4&port=80&code=000000");
        assert!(svg.contains("<svg"), "应含 <svg 标签");
        assert!(svg.len() > 100, "SVG 不应为空");
    }
}
