//! Touchpad pinch gestures.
//!
//! Wayland compositors report a pinch as a `zwp_pointer_gesture_pinch_v1` sequence. Winit
//! 0.30 only implements `PinchGesture` on macOS and iOS, so the Wayland backend reads the
//! protocol itself and emits the same `WindowEvent::PinchGesture` shape. The scale is
//! relative to the start of the gesture, so the event carries the change since the last
//! update.

use std::sync::Mutex;

use sctk::compositor::SurfaceData;
use sctk::globals::GlobalData;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_pointer::WlPointer;
use sctk::reexports::client::{delegate_dispatch, Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::{
    self, ZwpPointerGesturePinchV1,
};
use sctk::reexports::protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1;

use crate::event::{DeviceId, TouchPhase, WindowEvent};
use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::{self, DeviceId as WaylandDeviceId, WindowId};

/// The `zwp_pointer_gestures_v1` manager, when the compositor offers it.
pub struct PointerGesturesState {
    manager: ZwpPointerGesturesV1,
}

impl PointerGesturesState {
    /// Binds the gesture manager. A compositor without the global returns `BindError`.
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        // Version 1 has the pinch gesture; the later versions only add swipe/hold.
        let manager = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { manager })
    }

    /// Creates the pinch object for a seat's pointer.
    pub fn get_pinch(
        &self,
        pointer: &WlPointer,
        queue_handle: &QueueHandle<WinitState>,
    ) -> ZwpPointerGesturePinchV1 {
        self.manager.get_pinch_gesture(pointer, queue_handle, PinchData::new())
    }
}

impl Dispatch<ZwpPointerGesturesV1, GlobalData, WinitState> for PointerGesturesState {
    fn event(
        _state: &mut WinitState,
        _proxy: &ZwpPointerGesturesV1,
        _event: <ZwpPointerGesturesV1 as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
    }
}

/// State for one pinch object: the window the gesture started on and the last scale, because
/// the protocol reports the scale relative to the start of the gesture.
pub struct PinchData {
    inner: Mutex<PinchInner>,
}

struct PinchInner {
    window: Option<WindowId>,
    scale: f64,
}

impl PinchData {
    fn new() -> Self {
        Self {
            inner: Mutex::new(PinchInner {
                window: None,
                scale: 1.0,
            }),
        }
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, PinchData, WinitState> for PointerGesturesState {
    fn event(
        state: &mut WinitState,
        _proxy: &ZwpPointerGesturePinchV1,
        event: <ZwpPointerGesturePinchV1 as Proxy>::Event,
        data: &PinchData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
        let device_id = DeviceId(crate::platform_impl::DeviceId::Wayland(WaylandDeviceId));

        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { surface, .. } => {
                // Client-side decorations put the pointer on a frame surface; the window is
                // the one whose surface the gesture started on.
                let parent = surface
                    .data::<SurfaceData>()
                    .and_then(|data| data.parent_surface())
                    .unwrap_or(&surface);
                let window_id = wayland::make_wid(parent);
                tracing::debug!("pinch gesture started on window {window_id:?}");

                let mut inner = data.inner.lock().unwrap();
                inner.window = Some(window_id);
                inner.scale = 1.0;
                state.events_sink.push_window_event(
                    WindowEvent::PinchGesture {
                        device_id,
                        delta: 0.0,
                        phase: TouchPhase::Started,
                    },
                    window_id,
                );
            },
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. } => {
                let mut inner = data.inner.lock().unwrap();
                let Some(window_id) = inner.window else { return };
                let delta = if inner.scale > 0.0 {
                    scale / inner.scale - 1.0
                } else {
                    0.0
                };
                inner.scale = scale;

                if delta.abs() > f64::EPSILON {
                    state.events_sink.push_window_event(
                        WindowEvent::PinchGesture {
                            device_id,
                            delta,
                            phase: TouchPhase::Moved,
                        },
                        window_id,
                    );
                }
            },
            zwp_pointer_gesture_pinch_v1::Event::End { .. } => {
                let mut inner = data.inner.lock().unwrap();
                if let Some(window_id) = inner.window.take() {
                    state.events_sink.push_window_event(
                        WindowEvent::PinchGesture {
                            device_id,
                            delta: 0.0,
                            phase: TouchPhase::Ended,
                        },
                        window_id,
                    );
                }
                inner.scale = 1.0;
            },
            _ => (),
        }
    }
}

delegate_dispatch!(WinitState: [ZwpPointerGesturesV1: GlobalData] => PointerGesturesState);
delegate_dispatch!(WinitState: [ZwpPointerGesturePinchV1: PinchData] => PointerGesturesState);
