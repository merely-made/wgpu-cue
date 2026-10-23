// SPDX-License-Identifier: MPL-2.0
use crate::{
    document::{DocumentSession, SessionEffect},
    *,
};

/// Canonical retained input dispatch shared by standalone engine adapters and
/// application controllers. This owns no renderer, host routing or event queue.
/// A retained session cannot represent native key fields or scroll modifiers;
/// those are refused before changing its state.
pub fn dispatch_document_input<F: 'static>(
    session: &mut dyn DocumentSession<F>,
    input: Input,
) -> Result<SessionInputResult> {
    match input {
        Input::Document(input) => {
            let point = match &input {
                SessionInput::PointerMoved { x, y, .. }
                | SessionInput::PointerButton { x, y, .. } => Some((*x, *y)),
                _ => None,
            };
            if point.is_some_and(|(x, y)| !x.is_finite() || !y.is_finite()) {
                return Err(Error::Invalid("non-finite pointer position".into()));
            }
            Ok(session.input(input))
        }
        Input::Scroll {
            x,
            y,
            dx,
            dy,
            modifiers,
        } => {
            if ![x, y, dx, dy].iter().all(|value| value.is_finite()) {
                return Err(Error::Invalid("non-finite scroll".into()));
            }
            if modifiers != SessionModifiers::default() {
                return Err(Error::Unsupported(
                    "modified scroll is not represented by DocumentSession".into(),
                ));
            }
            let handled = session.scroll_at(x, y, dx, dy);
            Ok(SessionInputResult {
                effect: if handled {
                    SessionEffect::Handled
                } else {
                    SessionEffect::Ignored
                },
                cursor: None,
                capture: None,
                editable: session.editable_focus(),
            })
        }
        Input::NativeKey(_) => Err(Error::Unsupported(
            "send canonical logical keys and separate text/IME".into(),
        )),
    }
}

/// Ordered observations of retained input. Hosts use DocumentInput for capture,
/// editability and admission, and apply navigation/submission policy once from
/// the dedicated events (the canonical result still preserves its full effect).
pub fn document_input_events(result: SessionInputResult) -> std::vec::IntoIter<Event> {
    use crate::document::SessionCursor;
    let mut events = vec![Event::DocumentInput(result.clone())];
    if let Some(cursor) = result.cursor {
        let cursor = match cursor {
            SessionCursor::Default => "default",
            SessionCursor::Pointer => "pointer",
            SessionCursor::Text => "text",
        };
        events.push(Event::CursorChanged(cursor.into()));
    }
    match result.effect {
        SessionEffect::Navigate(address) => events.push(Event::NavigationRequested { address }),
        SessionEffect::Submit(form) => events.push(Event::FormSubmitted(form)),
        _ => {}
    }
    events.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_navigation_has_one_dedicated_event_after_input_and_cursor() {
        let result = SessionInputResult {
            effect: SessionEffect::Navigate("https://held.test/next".into()),
            cursor: Some(crate::document::SessionCursor::Pointer),
            capture: Some(false),
            editable: true,
        };
        let events = document_input_events(result.clone()).collect::<Vec<_>>();
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0],Event::DocumentInput(observed) if observed==&result));
        assert!(matches!(&events[1],Event::CursorChanged(cursor) if cursor=="pointer"));
        assert!(
            matches!(&events[2],Event::NavigationRequested{address} if address=="https://held.test/next")
        );
    }
}
