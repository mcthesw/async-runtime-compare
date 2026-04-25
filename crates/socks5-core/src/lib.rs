use std::fmt;

pub const VERSION: u8 = 0x05;
pub const METHOD_NO_AUTH: u8 = 0x00;
pub const CMD_CONNECT: u8 = 0x01;
pub const ATYP_IPV4: u8 = 0x01;
pub const ATYP_DOMAIN: u8 = 0x03;
pub const ATYP_IPV6: u8 = 0x04;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    pub target: TargetAddr,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetAddr {
    Ipv4([u8; 4]),
    Domain(String),
    Ipv6([u8; 16]),
}

impl ConnectRequest {
    pub fn target_string(&self) -> String {
        match self.target.clone() {
            TargetAddr::Ipv4(addr) => {
                format!(
                    "{}.{}.{}.{}:{}",
                    addr[0], addr[1], addr[2], addr[3], self.port
                )
            }
            TargetAddr::Domain(domain) => {
                format!("{domain}:{}", self.port)
            }
            TargetAddr::Ipv6(addr) => {
                let ip = std::net::Ipv6Addr::from(addr);
                format!("[{ip}]:{}", self.port)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocksError {
    ShortBuffer { needed: usize, actual: usize },
    InvalidVersion(u8),
    NoAcceptableAuthMethod,
    UnsupportedCommand(u8),
    InvalidReserved(u8),
    UnsupportedAddressType(u8),
    EmptyDomain,
    InvalidDomain,
    TrailingBytes,
}

impl fmt::Display for SocksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShortBuffer { needed, actual } => {
                write!(f, "short buffer: needed {needed} bytes, got {actual}")
            }
            Self::InvalidVersion(version) => write!(f, "invalid SOCKS version: {version:#04x}"),
            Self::NoAcceptableAuthMethod => write!(f, "no acceptable auth method"),
            Self::UnsupportedCommand(command) => write!(f, "unsupported command: {command:#04x}"),
            Self::InvalidReserved(rsv) => write!(f, "invalid reserved byte: {rsv:#04x}"),
            Self::UnsupportedAddressType(atyp) => {
                write!(f, "unsupported address type: {atyp:#04x}")
            }
            Self::EmptyDomain => write!(f, "empty domain name"),
            Self::InvalidDomain => write!(f, "invalid domain name"),
            Self::TrailingBytes => write!(f, "trailing bytes"),
        }
    }
}

impl std::error::Error for SocksError {}

pub fn select_no_auth(greeting: &[u8]) -> Result<(), SocksError> {
    require_min_len(greeting, 2)?;

    let method_count = greeting[1] as usize;
    let expected_len = 2 + method_count;
    require_exact_len(greeting, expected_len)?;

    if greeting[2..].contains(&METHOD_NO_AUTH) {
        Ok(())
    } else {
        Err(SocksError::NoAcceptableAuthMethod)
    }
}

pub fn parse_connect_request(buf: &[u8]) -> Result<ConnectRequest, SocksError> {
    require_min_len(buf, 4)?;

    if buf[0] != VERSION {
        return Err(SocksError::InvalidVersion(buf[0]));
    }

    if buf[1] != CMD_CONNECT {
        return Err(SocksError::UnsupportedCommand(buf[1]));
    }

    if buf[2] != 0x00 {
        return Err(SocksError::InvalidReserved(buf[2]));
    }

    match buf[3] {
        ATYP_IPV4 => parse_ipv4_connect_request(buf),
        ATYP_DOMAIN => parse_domain_connect_request(buf),
        ATYP_IPV6 => parse_ipv6_connect_request(buf),
        atyp => Err(SocksError::UnsupportedAddressType(atyp)),
    }
}

pub fn encode_method_selection_no_auth() -> [u8; 2] {
    [VERSION, METHOD_NO_AUTH]
}

pub fn encode_success_reply() -> [u8; 10] {
    encode_reply(0x00)
}

pub fn encode_general_failure_reply() -> [u8; 10] {
    encode_reply(0x01)
}

fn parse_ipv4_connect_request(buf: &[u8]) -> Result<ConnectRequest, SocksError> {
    require_exact_len(buf, 10)?;

    let mut addr = [0; 4];
    addr.copy_from_slice(&buf[4..8]);

    Ok(ConnectRequest {
        target: TargetAddr::Ipv4(addr),
        port: u16::from_be_bytes([buf[8], buf[9]]),
    })
}

fn parse_domain_connect_request(buf: &[u8]) -> Result<ConnectRequest, SocksError> {
    require_min_len(buf, 5)?;

    let domain_len = buf[4] as usize;
    if domain_len == 0 {
        return Err(SocksError::EmptyDomain);
    }

    let expected_len = 5 + domain_len + 2;
    require_exact_len(buf, expected_len)?;

    let domain_end = 5 + domain_len;
    let domain = std::str::from_utf8(&buf[5..domain_end])
        .map_err(|_| SocksError::InvalidDomain)?
        .to_owned();

    Ok(ConnectRequest {
        target: TargetAddr::Domain(domain),
        port: u16::from_be_bytes([buf[domain_end], buf[domain_end + 1]]),
    })
}

fn parse_ipv6_connect_request(buf: &[u8]) -> Result<ConnectRequest, SocksError> {
    require_exact_len(buf, 22)?;

    let mut addr = [0; 16];
    addr.copy_from_slice(&buf[4..20]);

    Ok(ConnectRequest {
        target: TargetAddr::Ipv6(addr),
        port: u16::from_be_bytes([buf[20], buf[21]]),
    })
}

fn encode_reply(rep: u8) -> [u8; 10] {
    [VERSION, rep, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0]
}

fn require_min_len(buf: &[u8], needed: usize) -> Result<(), SocksError> {
    if buf.len() < needed {
        Err(SocksError::ShortBuffer {
            needed,
            actual: buf.len(),
        })
    } else {
        Ok(())
    }
}

fn require_exact_len(buf: &[u8], needed: usize) -> Result<(), SocksError> {
    match buf.len().cmp(&needed) {
        std::cmp::Ordering::Less => Err(SocksError::ShortBuffer {
            needed,
            actual: buf.len(),
        }),
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Greater => Err(SocksError::TrailingBytes),
    }
}
