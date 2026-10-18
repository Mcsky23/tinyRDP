use anyhow::{Context, Result, anyhow};
use block2::RcBlock;
use objc2::{rc::autoreleasepool, runtime::Bool};
use objc2_foundation::{NSError, NSString, ns_string};
use objc2_local_authentication::{LAContext, LAError, LAErrorDomain, LAPolicy};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use uuid::Uuid;

// Stable across the tinyRDP rebrand so existing saved passwords stay available.
const SERVICE: &str = "com.minirdp.passwords";
const NOT_FOUND: i32 = -25300;

pub enum Lookup {
    Saved(String),
    Missing,
    Cancelled,
}

/// Call on a worker thread: authentication waits for the native macOS prompt.
/// Nothing is read from Keychain until macOS approves this connection.
pub fn get_authenticated(id: Uuid, device_name: &str) -> Result<Lookup> {
    autoreleasepool(|_| authenticated_lookup(|| authenticate(device_name), || read_password(id)))
}

fn authenticated_lookup(
    authorize: impl FnOnce() -> Result<bool>,
    read: impl FnOnce() -> Result<Option<String>>,
) -> Result<Lookup> {
    if !authorize()? {
        return Ok(Lookup::Cancelled);
    }
    Ok(match read()? {
        Some(password) => Lookup::Saved(password),
        None => Lookup::Missing,
    })
}

fn authenticate(device_name: &str) -> Result<bool> {
    // A new context per connection, with unlock reuse disabled. macOS owns all
    // biometric/password entry UI; tinyRDP never receives the Mac password.
    // SAFETY: LocalAuthentication is available on supported macOS versions.
    let context = unsafe { LAContext::new() };
    let policy = LAPolicy::DeviceOwnerAuthentication;
    unsafe {
        context.setTouchIDAuthenticationAllowableReuseDuration(0.0);
        context.setLocalizedFallbackTitle(Some(ns_string!("Use Mac Password")));
        context.setLocalizedCancelTitle(Some(ns_string!("Cancel")));
        context.canEvaluatePolicy_error(policy)
    }
    .map_err(|error| {
        anyhow!(
            "macOS authentication is unavailable: {}",
            error.localizedDescription()
        )
    })?;

    let (sender, receiver) = std::sync::mpsc::channel();
    let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
        // SAFETY: LocalAuthentication keeps the optional NSError valid for this
        // callback. Convert it to Rust data before leaving the framework queue.
        let error = unsafe { error.as_ref() };
        let result = if success.as_bool() {
            Ok(true)
        } else if let Some(error) = error {
            let cancelled = error.domain().isEqualToString(unsafe { LAErrorDomain })
                && matches!(
                    LAError(error.code()),
                    LAError::UserCancel | LAError::SystemCancel | LAError::AppCancel
                );
            if cancelled {
                Ok(false)
            } else {
                Err(anyhow!(
                    "macOS did not approve password access: {}",
                    error.localizedDescription()
                ))
            }
        } else {
            Err(anyhow!("macOS did not approve password access."))
        };
        let _ = sender.send(result);
    });
    let reason = NSString::from_str(&format!("access the saved password for {device_name}"));
    // SAFETY: The reply captures only a Send channel sender. The context and
    // block remain alive until the asynchronous evaluation finishes.
    unsafe { context.evaluatePolicy_localizedReason_reply(policy, &reason, &reply) };
    let result = receiver
        .recv()
        .context("macOS authentication ended without approval.");
    // SAFETY: Evaluation is complete. Never keep an authenticated context around
    // for another connection, including after a cancellation or error.
    unsafe { context.invalidate() };
    result?
}

fn read_password(id: Uuid) -> Result<Option<String>> {
    match get_generic_password(SERVICE, &id.to_string()) {
        Ok(bytes) => Ok(Some(
            String::from_utf8(bytes).context("The saved password is not valid UTF-8.")?,
        )),
        Err(error) if error.code() == NOT_FOUND => Ok(None),
        Err(error) => Err(error).context("Could not read the password from macOS Keychain."),
    }
}

pub fn set(id: Uuid, password: &str) -> Result<()> {
    set_generic_password(SERVICE, &id.to_string(), password.as_bytes())
        .context("Could not save the password in macOS Keychain.")
}

pub fn remove(id: Uuid) -> Result<()> {
    match delete_generic_password(SERVICE, &id.to_string()) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == NOT_FOUND => Ok(()),
        Err(error) => Err(error).context("Could not remove the password from macOS Keychain."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn authentication_finishes_before_keychain_is_read() {
        let authorized = Cell::new(false);
        let result = authenticated_lookup(
            || {
                authorized.set(true);
                Ok(true)
            },
            || {
                assert!(authorized.get());
                Ok(Some("test password".into()))
            },
        )
        .unwrap();
        assert!(matches!(result, Lookup::Saved(password) if password == "test password"));
    }

    #[test]
    fn cancellation_and_authentication_errors_never_read_keychain() {
        let cancelled = authenticated_lookup(
            || Ok(false),
            || panic!("Keychain must not be read after cancellation"),
        )
        .unwrap();
        assert!(matches!(cancelled, Lookup::Cancelled));
        let failed = authenticated_lookup(
            || Err(anyhow!("Authentication unavailable")),
            || panic!("Keychain must not be read after failed authentication"),
        );
        assert!(failed.is_err());
    }

    #[test]
    fn missing_password_is_not_treated_as_cancelled_authentication() {
        let result = authenticated_lookup(|| Ok(true), || Ok(None)).unwrap();
        assert!(matches!(result, Lookup::Missing));
    }
}
