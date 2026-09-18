//! Desktop-only "native emulator" — answers outbound envelopes on a
//! background thread so the same UI + plugin-client code path used by the
//! mobile shells also runs against a `Runtime::mock()` on desktop.
//!
//! Replace the placeholder match arms with your own plugin id handlers.

use std::sync::Arc;

use istmo::core::{Envelope, Frame, Runtime};

pub fn spawn(runtime: Arc<Runtime>, outbound: flume::Receiver<Envelope>) {
    std::thread::spawn(move || {
        while let Ok(env) = outbound.recv() {
            handle(&runtime, env);
        }
    });
}

fn handle(runtime: &Arc<Runtime>, envelope: Envelope) {
    match envelope.frame {
        Frame::Call { plugin_id, call_id, method, payload, .. } => {
            log::debug!(
                "emulator got Call plugin={plugin_id} method={method} payload={} bytes",
                payload.len()
            );

            // TODO: match on `plugin_id` + `method` and respond with a
            // bincode-encoded payload. Example:
            //
            // if plugin_id == "SergioRibera.example" && method == "get_value" {
            //     let response: Option<String> = Some("emulated".into());
            //     let bytes = bincode::encode_to_vec(&response, bincode::config::standard())
            //         .expect("encode response");
            //     runtime.inject_response(call_id, Ok(bytes)).ok();
            //     return;
            // }
            let _ = (call_id, runtime);
        }
        Frame::CreateInstance { .. } | Frame::DestroyInstance { .. } |
        Frame::Cancel { .. } | Frame::Notify { .. } |
        Frame::EarlyEvent { .. } | Frame::ReleaseNativeHandle { .. } => {
            log::debug!("emulator ignoring frame variant");
        }
        _ => {}
    }
}
