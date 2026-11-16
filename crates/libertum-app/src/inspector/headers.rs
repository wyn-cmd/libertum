//! Headers inspector: every heading in the document, indented by outline level, with the
//! page it starts on. Clicking a row moves the caret there.

use iced::advanced::text::Wrapping;
use iced::widget::{button, column, container, row, space, text};
use iced::{Element, Length, Padding};

use crate::app::Message;
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::divider;

use libertum_model::BlockId;

/// One row in the Headers pane. `page` is absent until the heading has been paginated.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadingEntry {
    pub block: BlockId,
    pub level: u8,
    pub text: String,
    pub page: Option<u32>,
}

pub fn header<'a>(t: Tokens) -> Element<'a, Message> {
    column![
        container(text(s::PANE_HEADERS).size(15.0).color(t.text)).padding(Padding {
            top: 14.0,
            bottom: 12.0,
            left: 14.0,
            right: 14.0,
        }),
        divider(t),
    ]
    .into()
}

pub fn body<'a>(
    headings: &'a [HeadingEntry],
    collapsed: &std::collections::HashSet<libertum_model::BlockId>,
    t: Tokens,
) -> Element<'a, Message> {
    if headings.is_empty() {
        return container(text(s::HEADERS_EMPTY).size(12.0).color(t.text_secondary))
            .padding(Padding {
                top: 14.0,
                bottom: 14.0,
                left: 14.0,
                right: 14.0,
            })
            .into();
    }
    let mut list = column![].spacing(1.0);
    let mut hidden_under: Option<u8> = None;
    for (index, heading) in headings.iter().enumerate() {
        let level = heading.level.max(1);
        if let Some(parent) = hidden_under {
            if level > parent {
                continue;
            }
            hidden_under = None;
        }
        let has_children = headings
            .get(index + 1)
            .is_some_and(|next| next.level.max(1) > level);
        let open = !collapsed.contains(&heading.block);
        let indent = 14.0 + f32::from(level.saturating_sub(1)) * 14.0;
        let arrow: Element<'a, Message> = if has_children {
            button(text(if open { "\u{25be}" } else { "\u{25b8}" }).size(11.0))
                .padding([0, 2])
                .width(Length::Fixed(16.0))
                .style(button::text)
                .on_press(Message::ToggleHeading(heading.block))
                .into()
        } else {
            space().width(Length::Fixed(16.0)).into()
        };
        let label = text(truncated(&heading.text, level))
            .size(if level == 1 { 13.0 } else { 12.0 })
            .width(Length::Fill)
            .wrapping(Wrapping::None)
            .color(if level == 1 { t.text } else { t.text_secondary });
        let page = container(
            text(
                heading
                    .page
                    .map(|page| page.to_string())
                    .unwrap_or_default(),
            )
            .size(11.0)
            .color(t.text_secondary),
        )
        .width(Length::Fixed(30.0))
        .align_x(iced::alignment::Horizontal::Right);
        list = list.push(
            button(
                row![arrow, label, page]
                    .width(Length::Fill)
                    .align_y(iced::Center),
            )
            .width(Length::Fill)
            .padding(Padding {
                top: 5.0,
                bottom: 5.0,
                left: indent,
                right: 14.0,
            })
            .style(button::text)
            .on_press(Message::GoToHeading(heading.block)),
        );
        if has_children && !open {
            hidden_under = Some(level);
        }
    }
    column![
        container(text(s::HEADERS_HINT).size(11.0).color(t.text_secondary)).padding(Padding {
            top: 10.0,
            bottom: 6.0,
            left: 14.0,
            right: 14.0,
        }),
        list,
    ]
    .into()
}

/// Sidebar headings have one line; a long chapter title gets an ellipsis before the page
/// number rather than pushing it out of place.
fn truncated(text: &str, level: u8) -> String {
    let level = usize::from(level.max(1));
    let max = 26usize.saturating_sub(level.saturating_sub(1) * 2).max(12);
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::truncated;

    #[test]
    fn long_headings_get_an_ellipsis() {
        assert_eq!(truncated("Short", 1), "Short");
        let long = "A very long chapter title that cannot fit in the sidebar at all";
        let cut = truncated(long, 1);
        assert!(cut.ends_with('\u{2026}'));
        assert!(cut.chars().count() <= 34);
        assert!(truncated(long, 3).chars().count() < cut.chars().count());
    }
}
