//! Core for the ZLT X28 PRO web UI: configuration, login and status queries.
//!
//! The firmware is a Vue/jQuery SPA talking to a single CGI endpoint:
//!
//!   POST https://192.168.1.1/cgi-bin/http.cgi   (JSON body, even for "GET" cmds)
//!
//! Sequence used by the browser, reproduced here in the smallest number of
//! round trips (the firmware replies HTTP/1.0 and closes the connection after
//! each response, so they cannot be pipelined):
//!
//!   1. cmd 232 GET_NEXT_LOGIN_TIME -> { token }
//!   2. cmd 100 LOGIN               -> { sessionId }   passwd = sha256(token + password)
//!   3. cmd 205 SYS_INFO_NETWORK    -> RF/network/band/operator
//!   4. cmd 207 DEVICE_INFO         -> temperature, cpu load, uptime
//!
//! No HTML/JS is downloaded and no cookie jar is needed — the sessionId travels
//! in the JSON body — and the router's self-signed certificate is accepted.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ureq::Agent;

/// TLS setup for the router's self-signed certificate.
///
/// The device serves HTTPS with a certificate that is not in any trust store,
/// so verification is skipped on purpose — this is a link-local appliance we
/// are intentionally logging into, not a public endpoint.
mod insecure_tls {
    use std::sync::Arc;

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    struct AcceptAnyCert;

    impl ServerCertVerifier for AcceptAnyCert {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    pub fn client_config() -> Arc<rustls::ClientConfig> {
        // builder_with_provider() avoids ambiguity when more than one rustls
        // crypto backend is compiled in.
        Arc::new(
            rustls::ClientConfig::builder_with_provider(
                rustls::crypto::ring::default_provider().into(),
            )
            .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
            .expect("ring supports TLS 1.2 and 1.3")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
            .with_no_client_auth(),
        )
    }
}

const CMD_GET_NEXT_LOGIN_TIME: u32 = 232;
const CMD_LOGIN: u32 = 100;
const CMD_RF_INFO: u32 = 205;
const CMD_DEVICE_INFO: u32 = 207;
/// System info, including the data connection's WAN address (`wanIP`).
const CMD_SYS_INFO: u32 = 0;

// Settings commands. Numbers and fields come from the X28 web UI as recorded
// by github.com/Ariiima/ZLT-X28-Pro (mode, bands, operator, AT) and
// github.com/ehsanecc/modempatch (reboot); every write carries a fresh token.
const CMD_REBOOT: u32 = 6;
const CMD_BAND_LOCK: u32 = 161;
const CMD_OPERATOR_SELECT: u32 = 228;
const CMD_WRITE_TOKEN: u32 = 233;
const CMD_NETWORK_MODE: u32 = 256;
const CMD_AT: u32 = 270;

const DEFAULT_HOST: &str = "https://192.168.1.1";

/// Everything the app needs to reach the router, persisted as JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub host: String,
    pub user: String,
    pub pass: String,
    /// Seconds between background refreshes.
    pub poll_secs: u64,
    /// Show the live temperature as menu bar text next to the icon.
    pub menu_title: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            user: "superadmin".to_string(),
            pass: "superadmin".to_string(),
            poll_secs: 30,
            menu_title: true,
        }
    }
}

impl Config {
    /// `~/.config/x28-pro/config.json`
    pub fn path() -> PathBuf {
        let base = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join(".config").join("x28-pro").join("config.json")
    }

    /// `~/.config/x28-pro/webui/` — where [`Client::capture_web_ui`] saves.
    pub fn web_ui_dir() -> PathBuf {
        Self::path().with_file_name("webui")
    }

    /// Load the saved config, then apply the `X28_HOST`/`X28_USER`/`X28_PASS`
    /// environment overrides (handy for the CLI).
    pub fn load() -> Config {
        let mut cfg = Self::load_from(&Self::path());

        if let Ok(v) = std::env::var("X28_HOST") {
            if !v.is_empty() {
                cfg.host = v;
            }
        }
        if let Ok(v) = std::env::var("X28_USER") {
            if !v.is_empty() {
                cfg.user = v;
            }
        }
        if let Ok(v) = std::env::var("X28_PASS") {
            if !v.is_empty() {
                cfg.pass = v;
            }
        }
        cfg.normalize();
        cfg
    }

    /// Read a config file, falling back to defaults when it is absent or
    /// unreadable (an older/corrupt file must not stop the app from starting).
    pub fn load_from(path: &std::path::Path) -> Config {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Config>(&raw).ok())
            .unwrap_or_default()
    }

    /// Make sure `host` has a scheme and no trailing slash.
    pub fn normalize(&mut self) {
        self.host = self.host.trim().trim_end_matches('/').to_string();
        if !self.host.starts_with("http://") && !self.host.starts_with("https://") {
            self.host = format!("https://{}", self.host);
        }
        self.poll_secs = self.poll_secs.clamp(5, 3600);
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, format!("{json}\n")).map_err(|e| format!("writing {}: {e}", path.display()))
    }
}

/// One snapshot of System Status > Device Information + RF state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    // cmd 207
    pub temperature: Option<String>,
    pub cpu_load: Option<String>,
    pub uptime_secs: Option<u64>,
    pub board: Option<String>,
    // cmd 205
    pub network_type: Option<String>,
    pub operator: Option<String>,
    pub signal_level: Option<String>,
    /// LTE band number reported by the firmware (`currentband`).
    pub band: Option<String>,
    /// NR band reported by the firmware (`currentband_5g`).
    pub band_5g: Option<String>,
    /// EARFCN / NR-ARFCN as reported in `FREQ` / `FREQ_5g`.
    pub earfcn: Option<String>,
    pub bandwidth: Option<String>,
    pub rsrp: Option<String>,
    pub rsrq: Option<String>,
    pub sinr: Option<String>,
    pub pci: Option<String>,
    pub cell_id: Option<String>,
    pub rsrp_5g: Option<String>,
    pub sinr_5g: Option<String>,
    pub pci_5g: Option<String>,
    pub earfcn_5g: Option<String>,
    pub bandwidth_5g: Option<String>,
    // cmd 0
    /// The data connection (APN) addresses; `None` if the router didn't say.
    pub wan: Option<WanInfo>,
}

impl Status {
    /// `70°C` / `—`
    pub fn temperature_label(&self) -> String {
        match &self.temperature {
            Some(t) => format!("{t}°C"),
            None => "—".to_string(),
        }
    }

    /// The firmware's network type as is: `4G`, `4G+`, `5G(NSA)`, `—`.
    pub fn network_label(&self) -> String {
        self.network_type.clone().unwrap_or_else(|| "—".to_string())
    }

    /// Generation tag from the network type: `5G(NSA)` → `5G`, `4G+` → `4G+`.
    pub fn generation(&self) -> Option<String> {
        let t = self.network_type.as_deref()?.trim();
        let mut chars = t.chars();
        let (digit, g) = (chars.next()?, chars.next()?);
        if !digit.is_ascii_digit() || !g.eq_ignore_ascii_case(&'G') {
            return None;
        }
        let plus = if chars.next() == Some('+') { "+" } else { "" };
        Some(format!("{digit}G{plus}"))
    }

    /// What the network type adds after the generation: `5G(NSA)` → `NSA`.
    pub fn network_detail(&self) -> Option<String> {
        let t = self.network_type.as_deref()?.trim();
        let rest = t.get(self.generation()?.len()..)?;
        let detail = rest.trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace());
        (!detail.is_empty()).then(|| detail.to_string())
    }

    /// `B1` for LTE, `n78` for NR, falling back to the LTE anchor band on NSA.
    /// With carrier aggregation every carrier is listed: `B7+B1+B3`.
    pub fn band_label(&self) -> String {
        let is_nr = self
            .network_type
            .as_deref()
            .map(|n| n.contains("5G") || n.contains("NR"))
            .unwrap_or(false);
        let label = |bands: &str, prefix: &str| {
            bands
                .split('+')
                .map(|b| format!("{prefix}{}", b.trim().trim_start_matches(['b', 'B', 'n', 'N'])))
                .collect::<Vec<_>>()
                .join("+")
        };
        if is_nr {
            if let Some(b) = self.band_5g.as_ref().filter(|b| !b.is_empty()) {
                return label(b, "n");
            }
        }
        match self.band.as_ref().filter(|b| !b.is_empty()) {
            Some(b) => label(b, "B"),
            None => "—".to_string(),
        }
    }

    /// Component carriers, primary first. With carrier aggregation the
    /// firmware joins each field's per-carrier values with `+` (`7+1+3`,
    /// `10+20+20`, ...); LTE carriers come before NR ones.
    pub fn carriers(&self) -> Vec<Carrier> {
        fn split(field: &Option<String>) -> Vec<Option<String>> {
            field
                .as_deref()
                .map(|f| {
                    f.split('+')
                        .map(|v| Some(v.trim().to_string()).filter(|v| !v.is_empty()))
                        .collect()
                })
                .unwrap_or_default()
        }
        let mut carriers = Vec::new();
        let radios = [
            (false, &self.band, &self.bandwidth, &self.earfcn, &self.pci),
            (true, &self.band_5g, &self.bandwidth_5g, &self.earfcn_5g, &self.pci_5g),
        ];
        for (nr, band, width, earfcn, pci) in radios {
            let (widths, earfcns, pcis) = (split(width), split(earfcn), split(pci));
            for (i, band) in split(band).into_iter().enumerate() {
                let Some(band) = band else { continue };
                let n = band.trim_start_matches(['b', 'B', 'n', 'N']);
                carriers.push(Carrier {
                    band: format!("{}{n}", if nr { "n" } else { "B" }),
                    nr,
                    width_mhz: widths.get(i).cloned().flatten(),
                    earfcn: earfcns.get(i).cloned().flatten(),
                    pci: pcis.get(i).cloned().flatten(),
                });
            }
        }
        carriers
    }

    /// `Irancell-GPRS · 4G · B1`
    pub fn summary(&self) -> String {
        let op = self
            .operator
            .as_deref()
            .filter(|o| !o.is_empty() && *o != "NULL")
            .unwrap_or("—");
        format!("{} · {} · {}", op, self.network_label(), self.band_label())
    }

    /// `RSRP −79 dBm · RSRQ −12 dB · SINR 5 dB · 20 MHz`
    pub fn rf_line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = self.rsrp.as_ref().filter(|v| !v.is_empty()) {
            parts.push(format!("RSRP {v} dBm"));
        }
        if let Some(v) = self.rsrq.as_ref().filter(|v| !v.is_empty()) {
            parts.push(format!("RSRQ {v} dB"));
        }
        if let Some(v) = self.sinr.as_ref().filter(|v| !v.is_empty()) {
            parts.push(format!("SINR {v} dB"));
        }
        if let Some(v) = self.bandwidth.as_ref().filter(|v| !v.is_empty()) {
            parts.push(format!("{v} MHz"));
        }
        if parts.is_empty() {
            "—".to_string()
        } else {
            parts.join(" · ")
        }
    }

    /// `2d 4h 32m` / `—`
    pub fn uptime_label(&self) -> String {
        match self.uptime_secs {
            Some(s) => format!("{}d {}h {}m", s / 86400, (s % 86400) / 3600, (s % 3600) / 60),
            None => "—".to_string(),
        }
    }

    pub fn temperature_c(&self) -> Option<f32> {
        number(&self.temperature)
    }

    pub fn rsrp_dbm(&self) -> Option<f32> {
        number(&self.rsrp)
    }

    pub fn rsrq_db(&self) -> Option<f32> {
        number(&self.rsrq)
    }

    pub fn sinr_db(&self) -> Option<f32> {
        number(&self.sinr)
    }

    pub fn rsrp_5g_dbm(&self) -> Option<f32> {
        number(&self.rsrp_5g)
    }

    pub fn sinr_5g_db(&self) -> Option<f32> {
        number(&self.sinr_5g)
    }

    /// Overall link grade: the worse of RSRP (coverage) and SINR (interference),
    /// using whichever of the two the firmware reported.
    pub fn link_quality(&self) -> Option<Quality> {
        let rsrp = self.rsrp_dbm().map(Quality::rsrp);
        let sinr = self.sinr_db().map(Quality::sinr);
        match (rsrp, sinr) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Signal strength on the firmware's own 0–5 scale (`signal_lvl`), the
    /// value the router's web UI draws as bars. Falls back to the RF grade
    /// when the firmware leaves it out.
    pub fn signal_bars(&self) -> Option<usize> {
        let firmware = self
            .signal_level
            .as_deref()
            .and_then(|l| l.trim().parse::<usize>().ok())
            .map(|l| l.min(5));
        firmware.or_else(|| {
            self.link_quality().map(|q| match q {
                Quality::Excellent => 5,
                Quality::Good => 4,
                Quality::Fair => 3,
                Quality::Poor => 1,
            })
        })
    }

    /// Attached to a 5G (NR) cell right now.
    pub fn on_nr(&self) -> bool {
        self.band_5g.is_some() || self.rsrp_5g.is_some()
    }
}

/// Leading number of a firmware field: `-79`, `-79 dBm`, `5.5` all parse.
fn number(field: &Option<String>) -> Option<f32> {
    let s = field.as_deref()?.trim();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    s[..end].parse().ok()
}

/// Four-step grade for an RF reading, on the thresholds commonly used for LTE.
/// Ordered worst to best so `min` picks the weaker of two readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    Poor,
    Fair,
    Good,
    Excellent,
}

impl Quality {
    /// Grade a reading where higher is better, given the lower bounds of
    /// Excellent, Good and Fair.
    fn grade(value: f32, [excellent, good, fair]: [f32; 3]) -> Self {
        if value >= excellent {
            Quality::Excellent
        } else if value >= good {
            Quality::Good
        } else if value >= fair {
            Quality::Fair
        } else {
            Quality::Poor
        }
    }

    pub fn rsrp(dbm: f32) -> Self {
        Self::grade(dbm, [-80.0, -90.0, -100.0])
    }

    pub fn rsrq(db: f32) -> Self {
        Self::grade(db, [-10.0, -15.0, -20.0])
    }

    pub fn sinr(db: f32) -> Self {
        Self::grade(db, [20.0, 13.0, 0.0])
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Excellent => "Excellent",
            Quality::Good => "Good",
            Quality::Fair => "Fair",
            Quality::Poor => "Poor",
        }
    }

    /// Lit bars on a 4-bar signal glyph.
    pub fn bars(self) -> usize {
        self as usize + 1
    }
}

/// Words for a 0–5 [`Status::signal_bars`] level.
pub fn signal_label(bars: usize) -> &'static str {
    match bars {
        5 => "Excellent",
        4 => "Good",
        3 => "Fair",
        2 => "Weak",
        1 => "Poor",
        _ => "No signal",
    }
}

/// Thermal band of the modem, for colouring the temperature readout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heat {
    Cool,
    Warm,
    Hot,
}

impl Heat {
    pub fn from_celsius(c: f32) -> Self {
        if c >= 70.0 {
            Heat::Hot
        } else if c >= 55.0 {
            Heat::Warm
        } else {
            Heat::Cool
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Heat::Cool => "Cool",
            Heat::Warm => "Warm",
            Heat::Hot => "Hot",
        }
    }
}

/// The mobile data connection: APN name, the addresses the network assigned
/// and its DNS servers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WanInfo {
    pub apn: Option<String>,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub dns: Vec<String>,
}

impl WanInfo {
    pub fn is_empty(&self) -> bool {
        self.apn.is_none() && self.ipv4.is_none() && self.ipv6.is_none() && self.dns.is_empty()
    }
}

/// Pick the connection details out of a cmd 0 reply by field name, since
/// firmware builds name them differently (`wanIP`, `wan_ipv6`, `dns1`,
/// `primary_dns`, `apn`, ...). Unset values (`0.0.0.0`, `::`) are skipped.
fn wan_info(reply: &Value) -> WanInfo {
    use std::net::IpAddr;

    let mut wan = WanInfo::default();
    let Some(fields) = reply.as_object() else {
        return wan;
    };
    // `wanIP` is the documented name; prefer it over pattern matches.
    if let Some(ip) = text(reply, "wanIP").filter(|v| v.parse::<std::net::Ipv4Addr>().is_ok_and(|a| !a.is_unspecified())) {
        wan.ipv4 = Some(ip);
    }
    for (key, value) in fields {
        let Some(value) = value.as_str().map(str::trim).filter(|v| !v.is_empty() && *v != "NULL") else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        let addrs: Vec<IpAddr> = value
            .split([',', ';', ' '])
            .filter_map(|v| v.trim().parse::<IpAddr>().ok())
            .filter(|a| !a.is_unspecified())
            .collect();
        if key.contains("dns") {
            for a in addrs {
                let a = a.to_string();
                if !wan.dns.contains(&a) {
                    wan.dns.push(a);
                }
            }
        } else if key.contains("apn") && addrs.is_empty() && !key.contains("enable") {
            wan.apn.get_or_insert_with(|| value.to_string());
        } else if key.contains("ip") && (key.contains("wan") || key.contains("v4") || key.contains("v6")) {
            for a in addrs {
                match a {
                    IpAddr::V4(_) if !key.contains("gateway") && !key.contains("mask") => {
                        wan.ipv4.get_or_insert_with(|| a.to_string());
                    }
                    IpAddr::V6(v6) if !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80 => {
                        // Skip link-local (fe80::/10): not a public address.
                        wan.ipv6.get_or_insert_with(|| a.to_string());
                    }
                    _ => {}
                }
            }
        }
    }
    wan
}

/// One component carrier of the connection (see [`Status::carriers`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carrier {
    /// `B7` / `n78`
    pub band: String,
    pub nr: bool,
    pub width_mhz: Option<String>,
    pub earfcn: Option<String>,
    pub pci: Option<String>,
}

/// A stateful session against one router. Cheap to build; it holds a pooled
/// TLS connection so repeated polls skip the handshake where the firmware
/// allows it.
pub struct Client {
    agent: Agent,
    host: String,
    user: String,
    pass: String,
    session: Option<String>,
}

impl Client {
    pub fn new(config: &Config) -> Self {
        Self::with_session(config, None)
    }

    pub fn with_session(config: &Config, session: Option<String>) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .tls_config(insecure_tls::client_config())
                .timeout_connect(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("Mozilla/5.0 (X28-Pro)")
                .build(),
            host: config.host.clone(),
            user: config.user.clone(),
            pass: config.pass.clone(),
            session,
        }
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// POST one command and parse the JSON reply. The firmware occasionally
    /// prefixes junk before the object, so we start at the first '{' just like
    /// the web UI does.
    fn rpc(&self, payload: &Value) -> Result<Value, String> {
        let body = payload.to_string();
        let url = format!("{}/cgi-bin/http.cgi", self.host);
        let response = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, */*")
            .send_bytes(body.as_bytes());

        let text = match response {
            Ok(r) => read_body(r)?,
            Err(ureq::Error::Status(code, r)) => {
                let body = read_body(r).unwrap_or_default();
                return Err(format!(
                    "HTTP {code}: {}",
                    body.chars().take(200).collect::<String>()
                ));
            }
            Err(e) => return Err(format!("request failed: {e}")),
        };

        // A write that changes nothing (e.g. re-sending the current band lock)
        // gets an empty body; the web UI treats that as success.
        if text.trim().is_empty() {
            return Ok(json!({ "success": true }));
        }
        let start = text
            .find('{')
            .ok_or_else(|| format!("non-JSON response: {text}"))?;
        serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<Value>()
            .next()
            .ok_or_else(|| format!("empty JSON reply: {text}"))?
            .map_err(|e| format!("bad JSON: {e}"))
    }

    fn login(&mut self) -> Result<(), String> {
        let challenge = self.rpc(&json!({
            "cmd": CMD_GET_NEXT_LOGIN_TIME,
            "method": "GET",
            "sessionId": "",
            "language": "CN",
        }))?;
        let token = challenge
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("no token in challenge: {challenge}"))?
            .to_string();

        let reply = self.rpc(&json!({
            "cmd": CMD_LOGIN,
            "method": "POST",
            "sessionId": client_session_id(),
            "username": self.user,
            "passwd": sha256_hex(&format!("{token}{}", self.pass)),
            "isAutoUpgrade": "0",
            "subcmd": 0,
            "language": "CN",
        }))?;

        if reply.get("AUTH").and_then(Value::as_str) != Some("AUTH") {
            self.session = None;
            let reason = reply
                .get("message")
                .or_else(|| reply.get("login_fail"))
                .map(|v| v.to_string())
                .unwrap_or_else(|| reply.to_string());
            return Err(format!("login failed: {reason}"));
        }
        self.session = reply
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if self.session.is_none() {
            return Err(format!("no sessionId in reply: {reply}"));
        }
        Ok(())
    }

    /// Run `op` with a session, logging in first (or re-logging in once after
    /// a session timeout) as needed.
    fn authed<T>(&mut self, op: impl Fn(&Self) -> Result<T, String>) -> Result<T, String> {
        if self.session.is_none() {
            self.login()?;
        }
        match op(self) {
            Err(e) if is_auth_error(&e) => {
                self.session = None;
                self.login()?;
                op(self)
            }
            other => other,
        }
    }

    /// Fetch one status snapshot.
    pub fn fetch(&mut self) -> Result<Status, String> {
        self.authed(Self::fetch_status)
    }

    /// Read a settings command. Replies carry no `success` flag, only data.
    fn get(&self, cmd: u32) -> Result<Value, String> {
        let reply = self.rpc(&json!({
            "cmd": cmd,
            "method": "GET",
            "sessionId": self.session.clone().unwrap_or_default(),
            "language": "CN",
        }))?;
        match reply.get("message").and_then(Value::as_str) {
            Some(m) if is_auth_error(m) => Err(m.to_string()),
            _ => Ok(reply),
        }
    }

    /// Write a settings command. Like the web UI, fetch a one-time token
    /// first; the router rejects writes without one.
    fn post(&self, cmd: u32, fields: Value) -> Result<Value, String> {
        let token = self
            .get(CMD_WRITE_TOKEN)?
            .get("token")
            .and_then(Value::as_str)
            .ok_or("router returned no write token")?
            .to_string();
        let mut payload = json!({
            "cmd": cmd,
            "method": "POST",
            "sessionId": self.session.clone().unwrap_or_default(),
            "token": token,
            "language": "CN",
        });
        if let (Some(dst), Value::Object(src)) = (payload.as_object_mut(), fields) {
            dst.extend(src);
        }
        let reply = self.rpc(&payload)?;
        check_ok(&reply, &format!("command {cmd}"))?;
        Ok(reply)
    }

    /// Send one AT command to the radio module and return its reply text.
    fn at(&self, command: &str) -> Result<String, String> {
        let reply = self.post(CMD_AT, json!({ "atInfo": base64(command.as_bytes()) }))?;
        Ok(reply.get("flag").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    /// Network mode, band lock and operator selection as the router has them.
    pub fn router_settings(&mut self) -> Result<RouterSettings, String> {
        self.authed(|c| {
            let mode = c.get(CMD_NETWORK_MODE)?;
            let bands = c.get(CMD_BAND_LOCK)?;
            let locked = |switch: &str, mask: &str| {
                (text(&bands, switch).as_deref() == Some("1"))
                    .then(|| mask_bands(&text(&bands, mask).unwrap_or_default()))
            };
            Ok(RouterSettings {
                network_mode: text(&mode, "networkMode").map(|m| m.to_uppercase()),
                bands: BandLock {
                    lte: locked("band_4g_switch", "lock_band_4g"),
                    nr: locked("band_5g_switch", "lock_band_5g"),
                },
                // Read through the modem's AT port; a failure here should not
                // hide the other two settings.
                operator: c.at("AT+COPS?").ok().and_then(|r| parse_cops(&r)),
                lte_supported: supported(&bands, "all_band_4g", &LTE_BANDS),
                nr_supported: supported(&bands, "all_band_5g", &NR_BANDS),
            })
        })
    }

    /// Set the radio access mode by its firmware code, e.g. `1C`.
    pub fn set_network_mode(&mut self, code: &str) -> Result<(), String> {
        self.authed(|c| c.post(CMD_NETWORK_MODE, json!({ "networkMode": code })).map(drop))
    }

    pub fn set_band_lock(&mut self, lock: &BandLock) -> Result<(), String> {
        let side = |bands: &Option<Vec<u32>>| match bands {
            Some(b) if !b.is_empty() => ("1", band_mask(b)),
            _ => ("0", String::new()),
        };
        let (lte_on, lte_mask) = side(&lock.lte);
        let (nr_on, nr_mask) = side(&lock.nr);
        self.authed(|c| {
            c.post(
                CMD_BAND_LOCK,
                json!({
                    "band4gRadio": lte_on,
                    "lock4gBand": lte_mask,
                    "band5gRadio": nr_on,
                    "lock5gBand": nr_mask,
                }),
            )
            .map(drop)
        })
    }

    /// Manual selection pins the modem to one network (no roaming fallback);
    /// automatic hands the choice back to the modem.
    pub fn set_operator(&mut self, selection: &OperatorSelection) -> Result<(), String> {
        self.authed(|c| match selection {
            OperatorSelection::Manual(plmn) => c
                .post(
                    CMD_OPERATOR_SELECT,
                    json!({ "plmn_select_cmd": "4", "plmn": plmn, "act": "7" }),
                )
                .map(drop),
            OperatorSelection::Automatic => c.at("AT+COPS=0").map(drop),
        })
    }

    pub fn reboot(&mut self) -> Result<(), String> {
        self.authed(|c| c.post(CMD_REBOOT, json!({ "rebootType": 1 })).map(drop))
    }

    /// Save the router's web UI (page + scripts) into `dir` and index every
    /// `cmd` number the scripts use, so settings beyond the ones above can be
    /// mapped. Needs no login: the UI is served as static files.
    pub fn capture_web_ui(&self, dir: &std::path::Path) -> Result<WebUiCapture, String> {
        fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let mut queue = vec!["/".to_string()];
        let mut seen = std::collections::BTreeSet::new();
        let mut files = Vec::new();
        let mut index = std::collections::BTreeMap::<u32, Vec<String>>::new();

        while let Some(path) = queue.pop() {
            if !seen.insert(path.clone()) || files.len() >= 80 {
                continue;
            }
            let url = format!("{}{}", self.host, path);
            let body = match self.agent.get(&url).call() {
                Ok(r) => read_body_limited(r, 16 << 20)?,
                Err(e) if path == "/" => return Err(format!("fetching {url}: {e}")),
                Err(_) => continue, // a guessed chunk path that does not exist
            };
            let name = if path == "/" {
                "index.html".to_string()
            } else {
                path.trim_start_matches('/').replace('/', "_")
            };
            fs::write(dir.join(&name), &body).map_err(|e| format!("writing {name}: {e}"))?;
            files.push(name);

            let base = path.rsplit_once('/').map(|(b, _)| b).unwrap_or("");
            for script in script_paths(&body) {
                queue.push(resolve_path(base, &script));
            }
            for (cmd, context) in cmd_mentions(&body) {
                let entry = index.entry(cmd).or_default();
                if entry.len() < 4 && !entry.contains(&context) {
                    entry.push(context);
                }
            }
        }

        let mut report = String::new();
        for (cmd, contexts) in &index {
            report.push_str(&format!("cmd {cmd}\n"));
            for c in contexts {
                report.push_str(&format!("    {c}\n"));
            }
        }
        fs::write(dir.join("cmd-index.txt"), report).map_err(|e| format!("writing index: {e}"))?;
        Ok(WebUiCapture {
            files: files.len(),
            commands: index.len(),
        })
    }

    fn fetch_status(&self) -> Result<Status, String> {
        let session = self.session.clone().unwrap_or_default();

        // RF/network/band — the whole picture in one ~80 ms request.
        let rf = self.rpc(&json!({
            "cmd": CMD_RF_INFO,
            "method": "GET",
            "sessionId": session,
            "language": "CN",
        }))?;
        check_ok(&rf, "network info")?;

        // Temperature/cpu/uptime — ~570 ms because the firmware stats the flash.
        let dev = self.rpc(&json!({
            "cmd": CMD_DEVICE_INFO,
            "method": "GET",
            "sessionId": session,
            "language": "CN",
        }))?;
        check_ok(&dev, "device info")?;

        // WAN address and DNS — optional extras: a failure here must not
        // cost the status reading.
        let wan = self
            .get(CMD_SYS_INFO)
            .ok()
            .map(|v| wan_info(&v))
            .filter(|w| !w.is_empty());

        let uptime_secs = text(&dev, "uptime")
            .and_then(|v| v.parse::<u64>().ok())
            .or_else(|| dev.get("uptime").and_then(Value::as_u64));

        Ok(Status {
            temperature: text(&dev, "device_temperature"),
            cpu_load: text(&dev, "cpu_usage"),
            uptime_secs,
            board: text(&dev, "board_type"),

            network_type: text(&rf, "network_type_str"),
            operator: text(&rf, "network_operator"),
            signal_level: text(&rf, "signal_lvl"),
            band: text(&rf, "currentband"),
            band_5g: text(&rf, "currentband_5g"),
            earfcn: text(&rf, "FREQ"),
            bandwidth: text(&rf, "bandwidth"),
            rsrp: text(&rf, "RSRP"),
            rsrq: text(&rf, "RSRQ"),
            sinr: text(&rf, "SINR"),
            pci: text(&rf, "PCI"),
            cell_id: text(&rf, "CELL_ID"),
            rsrp_5g: text(&rf, "RSRP_5G"),
            sinr_5g: text(&rf, "SINR_5G"),
            pci_5g: text(&rf, "PCI_5G"),
            earfcn_5g: text(&rf, "FREQ_5G"),
            bandwidth_5g: text(&rf, "bandwidth_5g"),
            wan,
        })
    }
}

/// Convenience for callers that keep no client around (the tray poller):
/// performs one fetch with an optional reusable session and hands the session
/// back for the next round.
pub fn fetch_once(config: &Config, session: Option<String>) -> (Result<Status, String>, Option<String>) {
    with_client(config, session, Client::fetch)
}

/// Run one operation on a short-lived client, handing the session back.
pub fn with_client<T>(
    config: &Config,
    session: Option<String>,
    op: impl FnOnce(&mut Client) -> Result<T, String>,
) -> (Result<T, String>, Option<String>) {
    let mut client = Client::with_session(config, session);
    let result = op(&mut client);
    (result, client.session().map(str::to_string))
}

/// Radio access mode as the firmware encodes it: a hex bit set where 4 = LTE,
/// 8 = NR non-standalone, 10 = NR standalone (20/40 split LTE into FDD/TDD).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkMode {
    Auto,
    NrSa,
    NrNsa,
    Lte,
    LteFdd,
    LteTdd,
}

impl NetworkMode {
    pub const ALL: [NetworkMode; 6] = [
        NetworkMode::Auto,
        NetworkMode::NrSa,
        NetworkMode::NrNsa,
        NetworkMode::Lte,
        NetworkMode::LteFdd,
        NetworkMode::LteTdd,
    ];

    pub fn code(self) -> &'static str {
        match self {
            NetworkMode::Auto => "1C",
            NetworkMode::NrSa => "10",
            NetworkMode::NrNsa => "C",
            NetworkMode::Lte => "4",
            NetworkMode::LteFdd => "20",
            NetworkMode::LteTdd => "40",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.code().eq_ignore_ascii_case(code.trim()))
    }

    pub fn label(self) -> &'static str {
        match self {
            NetworkMode::Auto => "5G + 4G (automatic)",
            NetworkMode::NrSa => "5G standalone only",
            NetworkMode::NrNsa => "5G non-standalone",
            NetworkMode::Lte => "4G only",
            NetworkMode::LteFdd => "4G FDD only",
            NetworkMode::LteTdd => "4G TDD only",
        }
    }
}

/// LTE bands of the X28 radio, used when the router doesn't list its own
/// (it reports them as `all_band_4g` in cmd 161).
pub const LTE_BANDS: [u32; 13] = [1, 3, 5, 7, 8, 20, 28, 32, 38, 40, 41, 42, 43];
/// Sub-6 NR bands of the X28 radio, as one router reported them in
/// `all_band_5g`; used when the router doesn't list its own.
pub const NR_BANDS: [u32; 12] = [1, 3, 5, 7, 8, 20, 28, 38, 40, 41, 77, 78];

/// Iranian networks offered for manual selection (PLMN, name).
pub const OPERATORS: [(&str, &str); 3] = [("43211", "MCI"), ("43235", "Irancell"), ("43220", "Rightel")];

pub fn operator_name(plmn: &str) -> Option<&'static str> {
    OPERATORS.iter().find(|(p, _)| *p == plmn).map(|(_, n)| *n)
}

/// Band lock per radio: `None` = all bands allowed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BandLock {
    pub lte: Option<Vec<u32>>,
    pub nr: Option<Vec<u32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperatorSelection {
    Automatic,
    /// Pinned to one PLMN, e.g. `43235`.
    Manual(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouterSettings {
    /// Upper-case firmware code; see [`NetworkMode::from_code`].
    pub network_mode: Option<String>,
    pub bands: BandLock,
    pub operator: Option<OperatorSelection>,
    /// Bands the radio supports, as the router reports them.
    pub lte_supported: Vec<u32>,
    pub nr_supported: Vec<u32>,
}

impl RouterSettings {
    /// The network mode lets the modem use 5G (see [`mode_allows_nr`]).
    pub fn allows_nr(&self) -> bool {
        self.network_mode.as_deref().is_some_and(mode_allows_nr)
    }
}

/// A network mode code (hex bit set) with the NR non-standalone (8) or
/// standalone (10) bit, i.e. one that lets the modem use 5G.
pub fn mode_allows_nr(code: &str) -> bool {
    u32::from_str_radix(code.trim(), 16).is_ok_and(|m| m & 0x18 != 0)
}

pub struct WebUiCapture {
    pub files: usize,
    pub commands: usize,
}

/// Hex bit mask the firmware uses for band locks: bit n-1 = band n.
pub fn band_mask(bands: &[u32]) -> String {
    let mask = bands
        .iter()
        .filter(|&&b| (1..=128).contains(&b))
        .fold(0u128, |m, &b| m | 1 << (b - 1));
    format!("{mask:x}")
}

pub fn mask_bands(hex: &str) -> Vec<u32> {
    let mask = u128::from_str_radix(hex.trim(), 16).unwrap_or(0);
    (0..128).filter(|i| mask >> i & 1 == 1).map(|i| i + 1).collect()
}

/// Supported bands from a cmd 161 mask field, or `fallback` if missing.
fn supported(reply: &Value, key: &str, fallback: &[u32]) -> Vec<u32> {
    match text(reply, key).map(|m| mask_bands(&m)) {
        Some(bands) if !bands.is_empty() => bands,
        _ => fallback.to_vec(),
    }
}

/// `+COPS: 0` → automatic, `+COPS: 1,2,"43235"` → manual (the firmware may
/// quote with ' or ").
fn parse_cops(reply: &str) -> Option<OperatorSelection> {
    let rest = &reply[reply.find("+COPS:")? + 6..];
    let mut parts = rest.trim().split(',');
    match parts.next()?.trim().chars().next()? {
        '0' => Some(OperatorSelection::Automatic),
        '1' | '4' => {
            let plmn = parts.nth(1)?;
            let plmn: String = plmn.chars().filter(char::is_ascii_digit).collect();
            (!plmn.is_empty()).then_some(OperatorSelection::Manual(plmn))
        }
        _ => None,
    }
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Script references in a page or bundle: `src="…js"` attributes and quoted
/// webpack chunk paths such as `"js/chunk-1a2b.js"`.
fn script_paths(body: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for quote in ['"', '\''] {
        for piece in body.split(quote).skip(1).step_by(2) {
            let ok = piece.ends_with(".js")
                && piece.len() < 160
                && !piece.contains(char::is_whitespace)
                && !piece.starts_with("http");
            if ok && !paths.iter().any(|p| p == piece) {
                paths.push(piece.to_string());
            }
        }
    }
    paths
}

/// Resolve a script path relative to the page that referenced it.
fn resolve_path(base: &str, path: &str) -> String {
    let path = path.trim_start_matches("./");
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{base}/{path}")
    }
}

/// Every `cmd: 123` / `"cmd":123` / `cmd=123` with ~140 chars of context.
fn cmd_mentions(body: &str) -> Vec<(u32, String)> {
    let bytes = body.as_bytes();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(i) = body[from..].find("cmd").map(|i| i + from) {
        from = i + 3;
        let mut j = from;
        while j < bytes.len() && matches!(bytes[j], b'"' | b'\'' | b' ') {
            j += 1;
        }
        if j >= bytes.len() || !matches!(bytes[j], b':' | b'=') {
            continue;
        }
        j += 1;
        while j < bytes.len() && matches!(bytes[j], b' ' | b'"' | b'\'') {
            j += 1;
        }
        let digits = bytes[j..].iter().take_while(|b| b.is_ascii_digit()).count();
        if let Ok(cmd) = body[j..j + digits].parse::<u32>() {
            let start = body.floor_char_boundary(i.saturating_sub(40));
            let end = body.ceil_char_boundary((j + 100).min(body.len()));
            found.push((cmd, body[start..end].replace('\n', " ")));
        }
    }
    found
}

fn check_ok(value: &Value, what: &str) -> Result<(), String> {
    if value.get("success").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let msg = value
        .get("message")
        .map(|m| m.to_string())
        .unwrap_or_else(|| value.to_string());
    if is_auth_error(&msg) {
        return Err(msg);
    }
    Err(format!("{what} request failed: {msg}"))
}

fn is_auth_error(message: &str) -> bool {
    message.contains("NO_AUTH") || message.contains("LOGIN_TIMEOUT")
}

/// Extract a non-empty string field (`NULL` is the firmware's null marker).
fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "NULL")
        .map(str::to_string)
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The firmware expects a 64-hex-char client session id (the browser builds
/// one from two MD5s of Math.random). Deriving it from time+pid+counter with
/// SHA-256 keeps us dependency-free while staying inside the expected format.
fn client_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!(
        "{nanos}:{}:{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    sha256_hex(&seed)
}

/// The firmware answers `HTTP/1.0` with no `Content-Length`, so the body is
/// delimited by the server closing the connection — and it closes the TCP
/// stream without a TLS `close_notify`. rustls reports that as
/// `UnexpectedEof` *after* handing us all the bytes; per the rustls guidance
/// we treat it as a plain end-of-stream. If the body really were truncated,
/// the JSON parse would fail, so a short read cannot pass silently.
fn read_body(response: ureq::Response) -> Result<String, String> {
    // 1 MiB is far beyond any reply of this API.
    read_body_limited(response, 1 << 20)
}

fn read_body_limited(response: ureq::Response, limit: usize) -> Result<String, String> {
    use std::io::Read;

    let mut reader = response.into_reader().take(limit as u64 + 1);
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("reading response: {e}")),
        }
        if buf.len() > limit {
            return Err("response too large".to_string());
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_through_json() {
        let mut cfg = Config {
            host: "192.168.1.1".into(),
            user: "admin".into(),
            pass: "secret".into(),
            poll_secs: 2,
            menu_title: false,
        };
        cfg.normalize();
        assert_eq!(cfg.host, "https://192.168.1.1");
        assert_eq!(cfg.poll_secs, 5, "poll interval is clamped to >= 5s");

        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);

        // Missing fields fall back to defaults instead of failing.
        let partial: Config = serde_json::from_str(r#"{"host":"http://10.0.0.1/"}"#).unwrap();
        assert_eq!(partial.poll_secs, Config::default().poll_secs);
        assert_eq!(partial.user, "superadmin");
    }

    #[test]
    fn config_round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("x28-pro-test-{}", std::process::id()));
        let path = dir.join("config.json");

        let mut cfg = Config {
            host: "10.0.0.1".into(),
            poll_secs: 45,
            ..Config::default()
        };
        cfg.normalize();
        cfg.save_to(&path).expect("write config");

        let loaded = Config::load_from(&path);
        assert_eq!(loaded, cfg);

        // A corrupt file must not stop the app from starting.
        fs::write(&path, "{not json").unwrap();
        assert_eq!(Config::load_from(&path), Config::default());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn band_labels_prefer_nr_on_5g() {
        let mut status = Status {
            network_type: Some("4G".into()),
            band: Some("3".into()),
            band_5g: Some("78".into()),
            ..Status::default()
        };
        assert_eq!(status.band_label(), "B3");

        status.network_type = Some("5G".into());
        assert_eq!(status.band_label(), "n78");

        status.band_5g = None;
        assert_eq!(status.band_label(), "B3", "NSA falls back to the anchor band");

        status.band = None;
        assert_eq!(status.band_label(), "—");
    }

    #[test]
    fn firmware_null_values_are_treated_as_missing() {
        let value = json!({"currentband": "", "RSRP": "NULL", "SINR": "5"});
        assert_eq!(text(&value, "currentband"), None);
        assert_eq!(text(&value, "RSRP"), None);
        assert_eq!(text(&value, "SINR"), Some("5".to_string()));
        assert_eq!(text(&value, "missing"), None);
    }

    #[test]
    fn uptime_formats_as_days_hours_minutes() {
        let status = Status {
            uptime_secs: Some(2 * 86400 + 4 * 3600 + 32 * 60),
            ..Status::default()
        };
        assert_eq!(status.uptime_label(), "2d 4h 32m");
        assert_eq!(Status::default().uptime_label(), "—");
    }

    #[test]
    fn numeric_fields_parse_with_units_and_signs() {
        let status = Status {
            temperature: Some("68".into()),
            rsrp: Some("-79 dBm".into()),
            rsrq: Some("-12.5".into()),
            sinr: Some("n/a".into()),
            ..Status::default()
        };
        assert_eq!(status.temperature_c(), Some(68.0));
        assert_eq!(status.rsrp_dbm(), Some(-79.0));
        assert_eq!(status.rsrq_db(), Some(-12.5));
        assert_eq!(status.sinr_db(), None);
        assert_eq!(Status::default().rsrp_dbm(), None);
    }

    #[test]
    fn link_quality_is_the_weaker_reading() {
        assert_eq!(Quality::rsrp(-79.0), Quality::Excellent);
        assert_eq!(Quality::rsrp(-95.0), Quality::Fair);
        assert_eq!(Quality::sinr(5.0), Quality::Fair);
        assert_eq!(Quality::sinr(-2.0).bars(), 1);
        assert_eq!(Quality::Excellent.bars(), 4);

        let mut status = Status {
            rsrp: Some("-79".into()),
            sinr: Some("5".into()),
            ..Status::default()
        };
        assert_eq!(status.link_quality(), Some(Quality::Fair));
        status.sinr = None;
        assert_eq!(status.link_quality(), Some(Quality::Excellent));
        status.rsrp = None;
        assert_eq!(status.link_quality(), None);
    }

    #[test]
    fn band_masks_match_the_firmware() {
        // Values read from a real X28 by the ZLT-X28-Pro author.
        assert_eq!(band_mask(&[38, 40, 41, 42, 43]), "7a000000000");
        assert_eq!(&mask_bands("7A0880800D5")[..4], &[1, 3, 5, 7]);
        assert_eq!(mask_bands(&band_mask(&[78])), vec![78], "NR bands need >64 bits");
        assert_eq!(mask_bands(""), Vec::<u32>::new());
        // all_band_5g as read from the user's X28.
        assert_eq!(mask_bands("3000000001A0080800D5"), NR_BANDS.to_vec());
    }

    #[test]
    fn carrier_aggregation_splits_into_carriers() {
        // As read from the user's X28 on 4G+.
        let status = Status {
            network_type: Some("4G+".into()),
            band: Some("7+1+3+7".into()),
            bandwidth: Some("10+20+20+20".into()),
            earfcn: Some("3000+100+1800+2850".into()),
            pci: Some("65+65+65+65".into()),
            ..Status::default()
        };
        let carriers = status.carriers();
        assert_eq!(carriers.len(), 4);
        assert_eq!(
            carriers[0],
            Carrier {
                band: "B7".into(),
                nr: false,
                width_mhz: Some("10".into()),
                earfcn: Some("3000".into()),
                pci: Some("65".into()),
            }
        );
        assert_eq!(carriers[3].earfcn.as_deref(), Some("2850"));
        assert_eq!(status.band_label(), "B7+B1+B3+B7");

        let single = Status { band: Some("1".into()), bandwidth: Some("20".into()), ..Status::default() };
        assert_eq!(single.carriers().len(), 1);
        assert_eq!(single.band_label(), "B1");
        assert!(Status::default().carriers().is_empty());
    }

    #[test]
    fn wan_info_is_picked_by_field_name() {
        let reply = json!({
            "success": true,
            "wanIP": "10.120.33.7",
            "wan_ipv6": "2a01:5ec0:1000::1",
            "ipv6_link": "fe80::1",
            "dns1": "10.44.8.8",
            "dns2": "10.44.8.8",
            "secondary_dns": "217.218.127.127",
            "ipv6_dns": "::",
            "apn": "mtnirancell",
            "apn_enable": "1",
            "lanIP": "192.168.1.1",
        });
        let wan = wan_info(&reply);
        assert_eq!(wan.ipv4.as_deref(), Some("10.120.33.7"));
        assert_eq!(wan.ipv6.as_deref(), Some("2a01:5ec0:1000::1"));
        assert_eq!(wan.apn.as_deref(), Some("mtnirancell"));
        assert_eq!(wan.dns, vec!["10.44.8.8", "217.218.127.127"]);

        let unset = json!({ "wanIP": "0.0.0.0", "dns1": "0.0.0.0" });
        assert!(wan_info(&unset).is_empty());
    }

    #[test]
    fn generation_tag_comes_from_the_network_type() {
        let with = |t: &str| Status { network_type: Some(t.into()), ..Status::default() };
        assert_eq!(with("5G(NSA)").generation().as_deref(), Some("5G"));
        assert_eq!(with("5G(NSA)").network_detail().as_deref(), Some("NSA"));
        assert_eq!(with("4G+").generation().as_deref(), Some("4G+"));
        assert_eq!(with("4G+").network_detail(), None);
        assert_eq!(with("4G").generation().as_deref(), Some("4G"));
        assert_eq!(with("NULL").generation(), None);
        assert_eq!(Status::default().generation(), None);
    }

    #[test]
    fn signal_bars_follow_the_firmware_level() {
        // Strong RSRP but low SINR: the RF grade says Fair, the router says 5.
        let mut status = Status {
            signal_level: Some("5".into()),
            rsrp: Some("-78".into()),
            sinr: Some("3".into()),
            ..Status::default()
        };
        assert_eq!(status.signal_bars(), Some(5));
        assert_eq!(signal_label(5), "Excellent");
        status.signal_level = None;
        assert_eq!(status.signal_bars(), Some(3), "falls back to the RF grade");
        assert_eq!(Status::default().signal_bars(), None);
    }

    #[test]
    fn nr_is_allowed_by_mode_bits() {
        let with = |m: &str| RouterSettings { network_mode: Some(m.into()), ..RouterSettings::default() };
        assert!(with("1C").allows_nr());
        assert!(with("C").allows_nr());
        assert!(with("10").allows_nr());
        assert!(!with("4").allows_nr());
        assert!(!with("40").allows_nr());
        assert!(!RouterSettings::default().allows_nr());
    }

    #[test]
    fn network_modes_round_trip_their_codes() {
        for mode in NetworkMode::ALL {
            assert_eq!(NetworkMode::from_code(mode.code()), Some(mode));
        }
        assert_eq!(NetworkMode::from_code("1c"), Some(NetworkMode::Auto));
        assert_eq!(NetworkMode::from_code("FF"), None);
    }

    #[test]
    fn operator_selection_parses_both_quote_styles() {
        assert_eq!(parse_cops("+COPS: 0\r\nOK"), Some(OperatorSelection::Automatic));
        assert_eq!(
            parse_cops("+COPS: 1,2,'43235',7"),
            Some(OperatorSelection::Manual("43235".into()))
        );
        assert_eq!(
            parse_cops("\r\n+COPS: 1,2,\"43211\",7\r\n"),
            Some(OperatorSelection::Manual("43211".into()))
        );
        assert_eq!(parse_cops("ERROR"), None);
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"AT+COPS?"), "QVQrQ09QUz8=");
    }

    #[test]
    fn web_ui_scan_finds_scripts_and_cmds() {
        let html = r#"<script src="./js/app.1a2b.js"></script><script src='/js/vendor.js'>"#;
        assert_eq!(script_paths(html), vec!["./js/app.1a2b.js", "/js/vendor.js"]);
        assert_eq!(resolve_path("", "./js/app.js"), "/js/app.js");
        assert_eq!(resolve_path("/js", "chunk.js"), "/js/chunk.js");

        let js = r#"a={cmd:161,band4gRadio:1};b={"cmd": 256};c="cmd=6";d.cmdline=1"#;
        let cmds: Vec<u32> = cmd_mentions(js).into_iter().map(|(c, _)| c).collect();
        assert_eq!(cmds, vec![161, 256, 6]);
    }

    #[test]
    fn heat_bands() {
        assert_eq!(Heat::from_celsius(48.0), Heat::Cool);
        assert_eq!(Heat::from_celsius(55.0), Heat::Warm);
        assert_eq!(Heat::from_celsius(72.0), Heat::Hot);
    }
}
