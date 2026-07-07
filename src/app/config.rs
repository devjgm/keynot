//! Maps presentation metadata onto player behavior: which tachyonfx
//! effects a [`Transition`] produces. The enums themselves live in
//! [`crate::markdown::metadata`] so the parser validates them.

use tachyonfx::{Effect, EffectTimer, Interpolation, Motion, fx};

use crate::markdown::Transition;
use crate::theme::Theme;

/// Effects for a slide change. Only some transitions animate the outgoing
/// slide.
pub trait TransitionEffects {
    /// Effect applied to the incoming slide.
    fn enter(self, theme: &Theme, forward: bool) -> Option<Effect>;
    /// Effect applied to the outgoing slide before the new one enters.
    fn exit(self, theme: &Theme, forward: bool) -> Option<Effect>;
}

impl TransitionEffects for Transition {
    fn enter(self, theme: &Theme, forward: bool) -> Option<Effect> {
        let timer = EffectTimer::from_ms(300, Interpolation::SineOut);
        match self {
            Transition::Slide => Some(fx::slide_in(
                push_motion(forward),
                10,
                0,
                theme.background,
                EffectTimer::from_ms(220, Interpolation::SineOut),
            )),
            Transition::Coalesce => Some(fx::coalesce(timer)),
            Transition::Fade => Some(fx::fade_from(theme.background, theme.background, timer)),
            Transition::Sweep => {
                let motion = if forward {
                    Motion::LeftToRight
                } else {
                    Motion::RightToLeft
                };
                Some(fx::sweep_in(motion, 10, 0, theme.background, timer))
            }
            Transition::None => None,
        }
    }

    fn exit(self, theme: &Theme, forward: bool) -> Option<Effect> {
        match self {
            Transition::Slide => Some(fx::slide_out(
                push_motion(forward),
                10,
                0,
                theme.background,
                EffectTimer::from_ms(160, Interpolation::SineIn),
            )),
            _ => None,
        }
    }
}

/// Direction cells travel in a push transition: navigating forward pushes
/// content out the left edge, navigating back pushes it out the right.
fn push_motion(forward: bool) -> Motion {
    if forward {
        Motion::RightToLeft
    } else {
        Motion::LeftToRight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_transition_has_no_effects() {
        assert!(Transition::None.enter(&Theme::dark(), true).is_none());
        assert!(Transition::None.exit(&Theme::dark(), true).is_none());
    }

    #[test]
    fn slide_transition_has_exit_and_enter() {
        assert!(Transition::Slide.enter(&Theme::dark(), true).is_some());
        assert!(Transition::Slide.exit(&Theme::dark(), true).is_some());
    }

    #[test]
    fn only_slide_has_an_exit_phase() {
        for t in [Transition::Coalesce, Transition::Fade, Transition::Sweep] {
            assert!(t.enter(&Theme::dark(), true).is_some());
            assert!(t.exit(&Theme::dark(), true).is_none());
        }
    }
}
