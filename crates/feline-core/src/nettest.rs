// SPDX-License-Identifier: GPL-3.0-or-later
//! The network test: can this computer reach Roblox, and how fast?
//!
//! For each address it times the name lookup (DNS) and a TCP connection
//! (best of three), which is close to a ping but works through firewalls
//! that drop pings. The last game server is pinged for real (Roblox's game
//! traffic is UDP, which can't be "connected" to); many servers ignore
//! pings, which isn't a problem by itself. A well-known non-Roblox address
//! is tested too, so "the internet is down" and "only Roblox is blocked"
//! can be told apart.

use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::system::{Check, CheckLevel};

/// One address's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    /// What it is, e.g. "Roblox website".
    pub label: String,
    pub host: String,
    /// Name lookup time, ms (`None`: the name didn't resolve).
    pub dns_ms: Option<f32>,
    /// Best connection time, ms (`None`: no connection).
    pub connect_ms: Option<f32>,
    /// Pings only: replies out of tries.
    pub replies: Option<(u32, u32)>,
    pub error: Option<String>,
}

impl Probe {
    pub fn level(&self) -> CheckLevel {
        match (self.connect_ms, self.replies) {
            (_, Some((0, _))) => CheckLevel::Ok, // no ping replies: normal for many servers
            (_, Some((got, of))) if got < of => CheckLevel::Warning,
            (Some(ms), _) if ms > 250.0 => CheckLevel::Warning,
            (Some(_), _) => CheckLevel::Ok,
            (None, _) => CheckLevel::Problem,
        }
    }

    /// "42 ms", "no connection", "no reply to pings".
    pub fn summary(&self) -> String {
        match (self.connect_ms, self.replies) {
            (_, Some((0, _))) => "doesn't answer pings (normal for many game servers)".to_owned(),
            (Some(ms), Some((got, of))) if got < of => {
                format!("{ms:.0} ms, {}% of pings lost", (of - got) * 100 / of)
            }
            (Some(ms), _) => {
                let feel = match ms {
                    m if m < 60.0 => "great",
                    m if m < 120.0 => "good",
                    m if m < 250.0 => "okay",
                    _ => "slow",
                };
                format!("{ms:.0} ms ({feel})")
            }
            (None, _) => self.error.clone().unwrap_or_else(|| "no connection".to_owned()),
        }
    }
}

/// The addresses the test tries: (label, host).
pub const TARGETS: &[(&str, &str)] = &[
    ("The internet (Cloudflare)", "one.one.one.one"),
    ("Roblox website", "www.roblox.com"),
    ("Signing in", "auth.roblox.com"),
    ("Joining games", "gamejoin.roblox.com"),
    ("Game content", "assetdelivery.roblox.com"),
    ("Roblox updates", "setup.rbxcdn.com"),
];

fn ms(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

/// Tests one host on port 443.
pub fn probe(label: &str, host: &str) -> Probe {
    let mut result = Probe {
        label: label.to_owned(),
        host: host.to_owned(),
        dns_ms: None,
        connect_ms: None,
        replies: None,
        error: None,
    };
    let started = Instant::now();
    let addresses: Vec<SocketAddr> = match (host, 443).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => {
            result.error = Some("the name doesn't resolve (DNS)".to_owned());
            return result;
        }
    };
    result.dns_ms = Some(ms(started.elapsed()));
    // A DNS filter "blocks" by answering 0.0.0.0 or 127.0.0.1.
    if let Some(a) = addresses.first()
        && (a.ip().is_unspecified() || a.ip().is_loopback())
    {
        result.error = Some(format!("blocked: the name points to {}", a.ip()));
        return result;
    }
    let Some(address) = addresses.iter().find(|a| a.is_ipv4()).or(addresses.first()).copied() else {
        result.error = Some("no address".to_owned());
        return result;
    };
    let mut best: Option<f32> = None;
    let mut last_error = None;
    for _ in 0..3 {
        let t = Instant::now();
        match TcpStream::connect_timeout(&address, Duration::from_secs(3)) {
            Ok(_) => best = Some(best.map_or(ms(t.elapsed()), |b| b.min(ms(t.elapsed())))),
            Err(e) => last_error = Some(e.kind()),
        }
    }
    result.connect_ms = best;
    if best.is_none() {
        result.error = Some(match last_error {
            Some(std::io::ErrorKind::TimedOut) => "no answer (timed out)".to_owned(),
            Some(std::io::ErrorKind::ConnectionRefused) => "refused".to_owned(),
            _ => "no connection".to_owned(),
        });
    }
    result
}

/// Pings a game server `tries` times.
pub fn ping_server(label: &str, ip: IpAddr, tries: u32) -> Probe {
    let mut got = 0;
    let mut total = 0.0;
    for _ in 0..tries {
        if let Some(rtt) = ping_once(ip) {
            got += 1;
            total += rtt;
        }
    }
    Probe {
        label: label.to_owned(),
        host: ip.to_string(),
        dns_ms: None,
        connect_ms: (got > 0).then(|| total / got as f32),
        replies: Some((got, tries)),
        error: None,
    }
}

/// One ping's round trip, ms.
fn ping_once(ip: IpAddr) -> Option<f32> {
    #[cfg(windows)]
    {
        match ip {
            IpAddr::V4(v4) => feline_win::stats::ping(v4.octets(), 1000).map(|ms| ms as f32),
            IpAddr::V6(_) => None,
        }
    }
    #[cfg(not(windows))]
    {
        // Unprivileged pings need the system's `ping` (it has the rights).
        let out = std::process::Command::new("ping")
            .args(["-c", "1", "-W", "1", "-n"])
            .arg(ip.to_string())
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        parse_ping_time(&String::from_utf8_lossy(&out.stdout))
    }
}

/// `time=23.4 ms` in ping's output.
pub fn parse_ping_time(text: &str) -> Option<f32> {
    let at = text.find("time=")? + 5;
    text[at..]
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()?
        .parse()
        .ok()
}

/// Runs the whole test. `server`: the last game server, with its region.
pub fn run(server: Option<(IpAddr, String)>) -> Vec<Probe> {
    let handles: Vec<_> = TARGETS
        .iter()
        .map(|&(label, host)| std::thread::spawn(move || probe(label, host)))
        .collect();
    let mut probes: Vec<Probe> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    if let Some((ip, region)) = server {
        let label = if region.is_empty() {
            "Your last game server".to_owned()
        } else {
            format!("Your last game server ({region})")
        };
        probes.push(ping_server(&label, ip, 5));
    }
    probes
}

/// What the results mean.
pub fn verdict(probes: &[Probe]) -> Check {
    let internet = probes.first().is_some_and(|p| p.connect_ms.is_some());
    let roblox: Vec<&Probe> = probes.iter().skip(1).filter(|p| p.replies.is_none()).collect();
    let failed: Vec<&str> = roblox
        .iter()
        .filter(|p| p.connect_ms.is_none())
        .map(|p| p.label.as_str())
        .collect();
    if !internet && failed.len() == roblox.len() {
        return Check::new(
            CheckLevel::Problem,
            "No internet connection",
            "Nothing could be reached. Check your Wi-Fi or cable, then try again.",
        );
    }
    if !failed.is_empty() {
        let blocked = roblox.iter().any(|p| p.error.as_deref().is_some_and(|e| e.starts_with("blocked")));
        return Check::new(
            CheckLevel::Problem,
            format!("Roblox can't be reached: {}", failed.join(", ")),
            if blocked {
                "A DNS filter, ad-blocker or the hosts file sends these addresses nowhere. Allow *.roblox.com and *.rbxcdn.com there."
            } else {
                "The internet works, but these Roblox addresses don't answer. A firewall, VPN or school/work network may block Roblox, or Roblox is having trouble (see its status on the Roblox page)."
            },
        );
    }
    let slow = roblox.iter().filter(|p| p.connect_ms.is_some_and(|m| m > 250.0)).count();
    let lossy = probes.iter().any(|p| matches!(p.replies, Some((got, of)) if got > 0 && got < of));
    if slow > 0 || lossy {
        return Check::new(
            CheckLevel::Warning,
            "Roblox is reachable, but slowly",
            "Expect lag. Use a cable instead of Wi-Fi if you can, pause big downloads and streams, and turn off a VPN while playing.",
        );
    }
    Check::new(CheckLevel::Ok, "Your connection to Roblox is purr-fect", "Everything answered quickly.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(connect: Option<f32>, replies: Option<(u32, u32)>, error: Option<&str>) -> Probe {
        Probe {
            label: "x".into(),
            host: "x".into(),
            dns_ms: Some(1.0),
            connect_ms: connect,
            replies,
            error: error.map(Into::into),
        }
    }

    #[test]
    fn results_are_explained() {
        assert_eq!(p(Some(30.0), None, None).summary(), "30 ms (great)");
        assert_eq!(p(None, Some((0, 5)), None).level(), CheckLevel::Ok);
        assert_eq!(p(Some(40.0), Some((4, 5)), None).summary(), "40 ms, 20% of pings lost");
        let good = vec![p(Some(10.0), None, None), p(Some(20.0), None, None)];
        assert_eq!(verdict(&good).level, CheckLevel::Ok);
        let blocked = vec![p(Some(10.0), None, None), p(None, None, Some("blocked: the name points to 0.0.0.0"))];
        assert!(verdict(&blocked).detail.contains("DNS filter"));
        let offline = vec![p(None, None, None), p(None, None, None)];
        assert_eq!(verdict(&offline).title, "No internet connection");
    }

    #[test]
    fn ping_output_parses() {
        assert_eq!(parse_ping_time("64 bytes from 1.1.1.1: icmp_seq=1 ttl=57 time=23.4 ms"), Some(23.4));
        assert_eq!(parse_ping_time("nothing"), None);
    }
}
