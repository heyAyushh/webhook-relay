use axum::extract::ConnectInfo;
use axum::http::HeaderMap;
use axum::http::request::Request;
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};
use tower_governor::errors::GovernorError;
use tower_governor::key_extractor::KeyExtractor;

const X_FORWARDED_FOR: &str = "x-forwarded-for";
const X_REAL_IP: &str = "x-real-ip";

#[derive(Debug, Clone)]
pub struct TrustedClientIpKeyExtractor {
    trust_proxy_headers: bool,
    trusted_proxy_cidrs: Vec<IpNet>,
}

impl TrustedClientIpKeyExtractor {
    pub fn new(trust_proxy_headers: bool, trusted_proxy_cidrs: Vec<IpNet>) -> Self {
        Self {
            trust_proxy_headers,
            trusted_proxy_cidrs,
        }
    }

    fn is_trusted_proxy(&self, peer_ip: IpAddr) -> bool {
        self.trusted_proxy_cidrs
            .iter()
            .any(|cidr| cidr.contains(&peer_ip))
    }
}

impl KeyExtractor for TrustedClientIpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        let peer_ip = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|addr| addr.ip())
            .or_else(|| req.extensions().get::<SocketAddr>().map(|addr| addr.ip()))
            .ok_or(GovernorError::UnableToExtractKey)?;

        if !self.trust_proxy_headers {
            return Ok(peer_ip);
        }

        if !self.is_trusted_proxy(peer_ip) {
            return Ok(peer_ip);
        }

        let headers = req.headers();
        parse_x_forwarded_for(headers)
            .or_else(|| parse_x_real_ip(headers))
            .or_else(|| parse_forwarded(headers))
            .or(Some(peer_ip))
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

fn parse_x_forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get(X_FORWARDED_FOR)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .split(',')
                .find_map(|part| part.trim().parse::<IpAddr>().ok())
        })
}

fn parse_x_real_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get(X_REAL_IP)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<IpAddr>().ok())
}

fn parse_forwarded(headers: &HeaderMap) -> Option<IpAddr> {
    headers.get("forwarded").and_then(|value| {
        value.to_str().ok().and_then(|raw| {
            raw.split(';').find_map(|segment| {
                let segment = segment.trim();
                if !segment.to_ascii_lowercase().starts_with("for=") {
                    return None;
                }
                let ip_text = segment
                    .split_once('=')
                    .map(|(_, value)| value.trim().trim_matches('"'))
                    .unwrap_or_default();
                let ip_only = ip_text
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split(':')
                    .next()
                    .unwrap_or_default();
                ip_only.parse::<IpAddr>().ok()
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    #[test]
    fn ignores_forwarded_headers_when_proxy_not_trusted() {
        let extractor = TrustedClientIpKeyExtractor::new(true, vec![]);
        let request = Request::builder()
            .header("x-forwarded-for", "1.2.3.4")
            .body(())
            .expect("request")
            .map(|_| ());

        let mut request = request;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 2], 1234))));

        assert_eq!(
            extractor.extract(&request).expect("extract"),
            IpAddr::from([10, 0, 0, 2])
        );
    }

    #[test]
    fn uses_x_forwarded_for_when_proxy_trusted() {
        let extractor = TrustedClientIpKeyExtractor::new(
            true,
            vec!["10.0.0.0/8".parse::<IpNet>().expect("cidr")],
        );
        let request = Request::builder()
            .header("x-forwarded-for", "1.2.3.4, 5.6.7.8")
            .body(())
            .expect("request")
            .map(|_| ());

        let mut request = request;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 0, 0, 2], 1234))));

        assert_eq!(
            extractor.extract(&request).expect("extract"),
            IpAddr::from([1, 2, 3, 4])
        );
    }
}
