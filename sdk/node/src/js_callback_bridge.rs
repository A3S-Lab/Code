//! Panic-safe bridge for synchronous JavaScript callbacks invoked through a
//! [`ThreadsafeFunction`](napi::threadsafe_function::ThreadsafeFunction).
//!
//! napi-rs treats an exception that escapes a TSFN callback as fatal while it
//! converts the callback's return value.  Always hand TSFN a JavaScript wrapper
//! that catches the host callback and returns a plain tagged envelope instead.

use napi::{Env, JsBoolean, JsFunction, JsObject, JsString, JsUnknown, ValueType};

const CALLBACK_WRAPPER_FACTORY: &str = r#"
(function a3sCreateSafeSyncCallback(callback) {
  function failure(error) {
    let message = "JavaScript callback failed";
    try {
      if (error !== null && error !== undefined) {
        if (typeof error.message === "string") {
          message = error.message;
        } else {
          message = String(error);
        }
      }
    } catch (_) {
      // Reading an adversarial thrown value must not make the wrapper throw.
    }
    return { __a3sSafeCallbackV1: true, ok: false, error: message };
  }

  return function a3sSafeSyncCallback(...args) {
    try {
      const value = callback.apply(this, args);

      // These SDK extension points are synchronous.  Treat thenables as a
      // controlled callback error and attach a rejection handler so an async
      // callback cannot create an unhandled rejection after native code has
      // already returned.
      if (value !== null &&
          (typeof value === "object" || typeof value === "function")) {
        let then;
        try {
          then = value.then;
        } catch (error) {
          return failure(error);
        }
        if (typeof then === "function") {
          try {
            then.call(value, undefined, function () {});
          } catch (_) {
            // The result is rejected below even if attaching a handler fails.
          }
          return failure(new TypeError("callback must return synchronously"));
        }
      }

      return { __a3sSafeCallbackV1: true, ok: true, value };
    } catch (error) {
      return failure(error);
    }
  };
})
"#;

/// A decoded return from the JavaScript safety wrapper.
pub(crate) enum JsCallbackOutcome {
    Returned(JsUnknown),
    Failed(String),
}

/// Wrap a user callback on the JavaScript thread before creating a TSFN.
///
/// The returned function is constructed entirely in JavaScript and never lets
/// a synchronous user exception cross the native callback boundary.
pub(crate) fn wrap_sync_callback(env: &Env, callback: JsFunction) -> napi::Result<JsFunction> {
    let factory: JsFunction = env.run_script(CALLBACK_WRAPPER_FACTORY)?;
    let wrapped = factory.call(None, &[callback])?;
    JsFunction::try_from(wrapped).map_err(|_| {
        napi::Error::from_reason("failed to construct the JavaScript callback safety wrapper")
    })
}

/// Decode the wrapper's plain object without allowing a conversion error to
/// escape a TSFN return callback.  Every malformed envelope becomes a regular
/// callback failure that callers can handle according to their API semantics.
pub(crate) fn decode_callback_outcome(value: JsUnknown) -> JsCallbackOutcome {
    let malformed = || {
        JsCallbackOutcome::Failed(
            "JavaScript callback bridge returned a malformed envelope".to_string(),
        )
    };

    if !matches!(value.get_type(), Ok(ValueType::Object)) {
        return malformed();
    }
    let object = unsafe { value.cast::<JsObject>() };

    let marker = match object.get_named_property::<JsBoolean>("__a3sSafeCallbackV1") {
        Ok(marker) => marker.get_value().unwrap_or(false),
        Err(_) => false,
    };
    if !marker {
        return malformed();
    }

    let ok = match object.get_named_property::<JsBoolean>("ok") {
        Ok(ok) => match ok.get_value() {
            Ok(ok) => ok,
            Err(_) => return malformed(),
        },
        Err(_) => return malformed(),
    };

    if ok {
        return object
            .get_named_property::<JsUnknown>("value")
            .map(JsCallbackOutcome::Returned)
            .unwrap_or_else(|_| malformed());
    }

    let message = object
        .get_named_property::<JsString>("error")
        .ok()
        .and_then(|error| error.into_utf8().ok())
        .and_then(|error| error.into_owned().ok())
        .unwrap_or_else(|| "JavaScript callback failed".to_string());
    JsCallbackOutcome::Failed(message)
}
