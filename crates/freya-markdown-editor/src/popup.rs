//! Telling the host where menus and pickers open.
//!
//! Hosts that route input outside freya, like a canvas taking stylus input
//! from a native backend, need to know which parts of the window a popup
//! covers so a tap on it does not reach what lies below.

use std::fmt;
use std::rc::Rc;

use freya::prelude::*;

/// Called with the window area of a popup the editor or its toolbar
/// opens, and with `None` once it closes. Provide it as context above the
/// editor and the toolbar.
#[derive(Clone)]
pub struct PopupReporter(Rc<dyn Fn(Option<Area>)>);

impl PopupReporter {
    #[must_use]
    pub fn new(report: impl Fn(Option<Area>) + 'static) -> Self {
        Self(Rc::new(report))
    }
}

impl PartialEq for PopupReporter {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for PopupReporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PopupReporter").finish_non_exhaustive()
    }
}

/// Popup content that keeps the [`PopupReporter`] in context, if any,
/// informed of its area while mounted.
#[derive(Clone, PartialEq)]
pub(crate) struct ReportedPopup {
    pub(crate) child: Element,
}

impl Component for ReportedPopup {
    fn render(&self) -> impl IntoElement {
        let reporter = use_try_consume::<PopupReporter>();
        use_drop({
            let reporter = reporter.clone();
            move || {
                if let Some(reporter) = reporter {
                    (reporter.0)(None);
                }
            }
        });
        rect()
            .on_sized(move |event: Event<SizedEventData>| {
                if let Some(reporter) = &reporter {
                    (reporter.0)(Some(event.area));
                }
            })
            .child(self.child.clone())
    }
}
