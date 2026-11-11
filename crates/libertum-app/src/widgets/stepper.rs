//! Numeric stepper: a text field with a unit label and small up/down arrows.
//!
//! The widget is stateless. The caller owns the value and the text being typed; clamping and
//! stepping live in [`StepSpec`] so they can be tested without a UI.

use iced::widget::{button, column, container, row, text, text_input};
use iced::{Border, Center, Element, Length, Padding};

use crate::icons::{Icon, icon};
use crate::theme::Tokens;

/// Range, increment and display precision of a numeric field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepSpec {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub decimals: usize,
}

impl StepSpec {
    pub const fn new(min: f32, max: f32, step: f32, decimals: usize) -> Self {
        Self {
            min,
            max,
            step,
            decimals,
        }
    }

    pub fn clamp(&self, value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(self.min, self.max)
        } else {
            self.min
        }
    }

    /// One step up (`direction > 0`) or down, snapped to the step grid and clamped.
    pub fn nudge(&self, value: f32, direction: i8) -> f32 {
        let snapped = (value / self.step).round() + f32::from(direction.signum());
        let factor = 10f32.powi(self.decimals as i32 + 1);
        let next = (snapped * self.step * factor).round() / factor;
        self.clamp(next)
    }

    pub fn format(&self, value: f32) -> String {
        format!("{value:.*}", self.decimals)
    }
}

/// Parse text typed into a stepper (accepts a decimal comma).
pub fn parse(text: &str) -> Option<f32> {
    text.trim()
        .replace(',', ".")
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}

const HEIGHT: f32 = 24.0;

/// A stepper. `value` is `None` for a mixed selection (shown as a dash).
#[allow(clippy::too_many_arguments)]
pub fn view<'a, M: Clone + 'a>(
    t: Tokens,
    spec: StepSpec,
    value: Option<f32>,
    unit: &'a str,
    editing: Option<&'a str>,
    on_text: impl Fn(String) -> M + 'a,
    on_submit: M,
    on_step: impl Fn(i8) -> M + 'a,
) -> Element<'a, M> {
    let shown = match (editing, value) {
        (Some(buffer), _) => buffer.to_owned(),
        (None, Some(v)) => spec.format(v),
        (None, None) => String::new(),
    };
    let input = text_input("\u{2014}", &shown)
        .on_input(on_text)
        .on_submit(on_submit)
        .size(13)
        .padding(Padding {
            top: 3.0,
            bottom: 3.0,
            left: 6.0,
            right: 2.0,
        })
        .width(Length::Fill)
        .style(move |_, _| text_input::Style {
            background: iced::Color::TRANSPARENT.into(),
            border: Border::default(),
            icon: t.text_secondary,
            placeholder: t.text_secondary,
            value: t.text,
            selection: iced::Color {
                a: 0.35,
                ..t.accent
            },
        });

    let at_max = value.is_some_and(|v| v >= spec.max);
    let at_min = value.is_some_and(|v| v <= spec.min);
    let arrows = column![
        arrow(t, Icon::ChevronUp, !at_max, on_step(1)),
        arrow(t, Icon::ChevronDown, !at_min, on_step(-1))
    ]
    .spacing(0);

    let mut body = row![input].align_y(Center).spacing(2);
    if !unit.is_empty() {
        body = body.push(text(unit).size(12).color(t.text_secondary));
    }
    body = body.push(arrows);

    container(body.padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 0.0,
        right: 3.0,
    }))
    .height(Length::Fixed(HEIGHT))
    .width(Length::Fill)
    .center_y(Length::Fixed(HEIGHT))
    .style(move |_| container::Style {
        background: Some(t.control.into()),
        border: Border {
            color: t.control_border,
            width: 1.0,
            radius: 5.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

fn arrow<'a, M: Clone + 'a>(t: Tokens, glyph: Icon, enabled: bool, on_press: M) -> Element<'a, M> {
    let colour = if enabled {
        t.text_secondary
    } else {
        t.text_disabled
    };
    button(
        container(icon(glyph, 9.0, colour))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(16.0))
    .height(Length::Fixed(HEIGHT / 2.0 - 1.0))
    .padding(0)
    .style(move |_, status| flat(t, status))
    .on_press(on_press)
    .into()
}

fn flat(t: Tokens, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => Some(t.hover.into()),
        button::Status::Pressed => Some(t.pressed.into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: t.text,
        border: Border {
            radius: 3.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: StepSpec = StepSpec::new(1.0, 10.0, 0.5, 1);

    #[test]
    fn clamps_to_the_range() {
        assert_eq!(SPEC.clamp(0.0), 1.0);
        assert_eq!(SPEC.clamp(99.0), 10.0);
        assert_eq!(SPEC.clamp(f32::NAN), 1.0);
        assert_eq!(SPEC.clamp(4.2), 4.2);
    }

    #[test]
    fn nudges_by_one_step_and_stops_at_the_ends() {
        assert_eq!(SPEC.nudge(2.0, 1), 2.5);
        assert_eq!(SPEC.nudge(2.0, -1), 1.5);
        assert_eq!(SPEC.nudge(10.0, 1), 10.0);
        assert_eq!(SPEC.nudge(1.0, -1), 1.0);
    }

    #[test]
    fn nudge_snaps_an_off_grid_value() {
        assert_eq!(SPEC.nudge(2.2, 1), 2.5);
    }

    #[test]
    fn formats_with_the_requested_precision() {
        assert_eq!(SPEC.format(2.0), "2.0");
        assert_eq!(StepSpec::new(0.0, 9.0, 1.0, 0).format(12.0), "12");
    }

    #[test]
    fn parses_typed_text() {
        assert_eq!(parse(" 2.5 "), Some(2.5));
        assert_eq!(parse("2,5"), Some(2.5));
        assert_eq!(parse("abc"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("inf"), None);
    }
}
