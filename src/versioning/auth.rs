use super::*;
use async_trait::async_trait;
use std::path::Path;
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    async fn resolve(
        &self,
        request: AuthRequest,
        cancel: CancellationToken,
    ) -> R<CredentialMaterial>;
}
pub struct SystemCredentials;
#[async_trait]
impl CredentialProvider for SystemCredentials {
    async fn resolve(
        &self,
        request: AuthRequest,
        cancel: CancellationToken,
    ) -> R<CredentialMaterial> {
        crate::workspace::check_cancel(&cancel)?;
        validate_url(&request.url)?;
        tokio::task::spawn_blocking(move || {
            let secret = request
                .config
                .credential_ref
                .as_ref()
                .map(|name| {
                    keyring::Entry::new("lumen", name)
                        .and_then(|e| e.get_password())
                        .map_err(|_| E::AuthRequired)
                })
                .transpose()?;
            if request.url.starts_with("https://") {
                Ok(secret
                    .map(|token| CredentialMaterial::Https {
                        username: "git".into(),
                        token,
                    })
                    .unwrap_or_default())
            } else if request.url.starts_with("ssh://") || is_scp(&request.url) {
                let authority = request.url.strip_prefix("ssh://").unwrap_or(&request.url);
                let username = authority
                    .split_once('@')
                    .map(|(u, _)| u)
                    .unwrap_or("git")
                    .to_string();
                Ok(if let Some(private_key) = request.config.ssh_private_key {
                    CredentialMaterial::Ssh {
                        username,
                        private_key,
                        passphrase: secret,
                    }
                } else {
                    CredentialMaterial::Agent { username }
                })
            } else {
                Ok(CredentialMaterial::Anonymous)
            }
        })
        .await
        .map_err(|_| E::AuthRequired)?
    }
}
fn is_scp(url: &str) -> bool {
    !url.contains("://") && url.contains('@') && url.contains(':')
}
pub(crate) fn validate_url(url: &str) -> R<()> {
    if url.is_empty() || url.starts_with('-') || url.contains('\0') {
        return Err(E::Configuration("invalid remote URL".into()));
    }
    if url.starts_with("https://") || url.starts_with("ssh://") {
        let parsed =
            reqwest::Url::parse(url).map_err(|_| E::Configuration("invalid remote URL".into()))?;
        if parsed.host_str().is_none()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.scheme() == "https" && !parsed.username().is_empty()
        {
            return Err(E::Configuration(
                "store credentials in the credential provider, not the remote URL".into(),
            ));
        }
        return Ok(());
    }
    if is_scp(url) {
        return Ok(());
    }
    if url.contains("://") || url.contains("::") {
        return Err(E::Configuration("unsupported remote transport".into()));
    }
    if !Path::new(url).is_absolute() {
        return Err(E::Configuration(
            "local remote path must be absolute".into(),
        ));
    }
    Ok(())
}
pub(crate) fn callbacks<'a>(
    material: CredentialMaterial,
    cancel: &'a CancellationToken,
    url: &str,
) -> git2::RemoteCallbacks<'a> {
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.transfer_progress(move |_| !cancel.is_cancelled());
    callbacks.sideband_progress(move |_| !cancel.is_cancelled());
    callbacks.push_transfer_progress(move |_, _, _| {});
    callbacks.push_negotiation(move |_| {
        if cancel.is_cancelled() {
            Err(git2::Error::new(
                git2::ErrorCode::User,
                git2::ErrorClass::Callback,
                "cancelled",
            ))
        } else {
            Ok(())
        }
    });
    callbacks.credentials(move |_, username, allowed| {
        if cancel.is_cancelled() {
            return Err(git2::Error::new(
                git2::ErrorCode::User,
                git2::ErrorClass::Callback,
                "cancelled",
            ));
        }
        if allowed.contains(git2::CredentialType::USERNAME) {
            return git2::Cred::username(username.unwrap_or("git"));
        }
        match &material {
            CredentialMaterial::Https { username, token }
                if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) =>
            {
                git2::Cred::userpass_plaintext(username, token)
            }
            CredentialMaterial::Ssh {
                username,
                private_key,
                passphrase,
            } if allowed.contains(git2::CredentialType::SSH_KEY) => {
                git2::Cred::ssh_key(username, None, private_key, passphrase.as_deref())
            }
            CredentialMaterial::Agent { username }
                if allowed.contains(git2::CredentialType::SSH_KEY) =>
            {
                git2::Cred::ssh_key_from_agent(username)
            }
            _ => Err(git2::Error::new(
                git2::ErrorCode::Auth,
                git2::ErrorClass::Callback,
                "credentials required",
            )),
        }
    });
    let ssh = url.starts_with("ssh://") || is_scp(url);
    let port = url
        .strip_prefix("ssh://")
        .and_then(|s| s.split('/').next())
        .and_then(|a| a.rsplit_once(':'))
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(22);
    callbacks.certificate_check(move |cert, host| {
        if !ssh {
            return Ok(git2::CertificateCheckStatus::CertificatePassthrough);
        }
        let verify = (|| {
            let key = cert.as_hostkey()?.hostkey()?;
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
            let path = Path::new(&home).join(".ssh/known_hosts");
            Some(verify_known_host(&path, host, port, key))
        })();
        if verify == Some(true) {
            Ok(git2::CertificateCheckStatus::CertificateOk)
        } else {
            Err(git2::Error::new(
                git2::ErrorCode::Certificate,
                git2::ErrorClass::Ssh,
                "host key not trusted",
            ))
        }
    });
    callbacks
}

// libgit2 already links libssh2. Keep the C lifetime and allocation boundary here.
fn verify_known_host(path: &Path, host: &str, port: u16, key: &[u8]) -> bool {
    use libssh2_sys as sys;
    use std::ffi::CString;
    let Ok(host) = CString::new(host) else {
        return false;
    };
    let Ok(lines) = std::fs::read(path) else {
        return false;
    };
    let Some(length) = key
        .get(..4)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_be_bytes)
    else {
        return false;
    };
    let Some(kind) = key.get(4..4usize.saturating_add(length as usize)) else {
        return false;
    };
    let key_type = match kind {
        b"ssh-rsa" => sys::LIBSSH2_KNOWNHOST_KEY_SSHRSA,
        b"ssh-dss" => sys::LIBSSH2_KNOWNHOST_KEY_SSHDSS,
        b"ecdsa-sha2-nistp256" => sys::LIBSSH2_KNOWNHOST_KEY_ECDSA_256,
        b"ecdsa-sha2-nistp384" => sys::LIBSSH2_KNOWNHOST_KEY_ECDSA_384,
        b"ecdsa-sha2-nistp521" => sys::LIBSSH2_KNOWNHOST_KEY_ECDSA_521,
        b"ssh-ed25519" => sys::LIBSSH2_KNOWNHOST_KEY_ED25519,
        _ => return false,
    };
    sys::init();
    // SAFETY: each allocation is checked, used only on this thread, then freed in reverse
    // order. All C strings/slices outlive their calls; no pointer escapes this function.
    unsafe {
        let session = sys::libssh2_session_init_ex(None, None, None, std::ptr::null_mut());
        if session.is_null() {
            return false;
        }
        let hosts = sys::libssh2_knownhost_init(session);
        if hosts.is_null() {
            sys::libssh2_session_free(session);
            return false;
        }
        for line in lines.split(|b| *b == b'\n') {
            if line.is_empty() || line.starts_with(b"#") {
                continue;
            }
            // The native parser reads a terminator even when an explicit length is given.
            let Ok(line) = CString::new(line) else {
                continue;
            };
            sys::libssh2_knownhost_readline(
                hosts,
                line.as_ptr(),
                line.as_bytes().len(),
                sys::LIBSSH2_KNOWNHOST_FILE_OPENSSH,
            );
        }
        let result = sys::libssh2_knownhost_checkp(
            hosts,
            host.as_ptr(),
            port as i32,
            key.as_ptr().cast(),
            key.len(),
            sys::LIBSSH2_KNOWNHOST_TYPE_PLAIN | sys::LIBSSH2_KNOWNHOST_KEYENC_RAW | key_type,
            std::ptr::null_mut(),
        );
        sys::libssh2_knownhost_free(hosts);
        sys::libssh2_session_free(session);
        result == sys::LIBSSH2_KNOWNHOST_CHECK_MATCH
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_hosts_accepts_only_the_matching_host_and_key() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("known_hosts");
        std::fs::write(&file,"example.test ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH\n").unwrap();
        let mut key = Vec::new();
        key.extend_from_slice(&11u32.to_be_bytes());
        key.extend_from_slice(b"ssh-ed25519");
        key.extend_from_slice(&32u32.to_be_bytes());
        key.extend_from_slice(&[7; 32]);
        assert!(verify_known_host(&file, "example.test", 22, &key));
        assert!(!verify_known_host(&file, "other.test", 22, &key));
        *key.last_mut().unwrap() = 8;
        assert!(!verify_known_host(&file, "example.test", 22, &key));
        assert!(!verify_known_host(&file, "example.test", 22, b"invalid"));
    }
}
