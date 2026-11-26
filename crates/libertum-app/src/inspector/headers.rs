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
    sidebar_width: f32,
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
        let size = if level == 1 { 13.0 } else { 12.0 };
        let budget = label_budget(sidebar_width, indent);
        // Clipped to its own box as well, so a misjudged glyph can never reach the page number.
        let label = container(
            text(fit(&heading.text, size, budget))
                .size(size)
                .wrapping(Wrapping::None)
                .color(if level == 1 { t.text } else { t.text_secondary }),
        )
        .width(Length::Fill)
        .clip(true);
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
        .width(Length::Fixed(PAGE_COLUMN))
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
    column![list,].into()
}

/// Width of the page-number column.
const PAGE_COLUMN: f32 = 30.0;
/// Width of the expand arrow beside each row.
const ARROW: f32 = 16.0;
/// Right padding of a row, inside its button.
const ROW_RIGHT: f32 = 14.0;
/// Clear space kept between a title and the page number.
const GAP: f32 = 8.0;
/// Room the scroll bar takes (its width plus margins), so a title never runs under it.
const SCROLL_BAR: f32 = 10.0;
/// The panel's 1px edge, which the sidebar width includes.
const EDGE: f32 = 1.0;

/// The width in pixels a row's title may use: the sidebar's width less everything else on
/// the row (the indent, arrow, page column, padding, scroll bar and a gap before the page).
fn label_budget(sidebar_width: f32, indent: f32) -> f32 {
    let taken = EDGE + SCROLL_BAR + indent + ARROW + GAP + PAGE_COLUMN + ROW_RIGHT;
    (sidebar_width - taken).max(0.0)
}

/// The width of `c` in ems, deliberately on the wide side: the sidebar font's real metrics
/// are not measured here, and a title that is cut a little early costs less than one that
/// reaches the page number.
fn glyph_em(c: char) -> f32 {
    match c {
        ' ' | 'i' | 'j' | 'l' | 'I' | '.' | ',' | ':' | ';' | '\'' | '!' | '|' | '`' => 0.34,
        'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '-' | '"' | '/' | '\\' => 0.42,
        'm' | 'w' | 'M' | 'W' | '@' | '%' | '\u{2014}' => 0.95,
        c if c.is_ascii_uppercase() => 0.72,
        c if c.is_ascii() => 0.58,
        // CJK, full-width forms and most symbols fill the em.
        c if ('\u{2e80}'..='\u{ffef}').contains(&c) => 1.05,
        _ => 0.72,
    }
}

/// An estimate of `text`'s width at `size` px.
fn estimate(text: &str, size: f32) -> f32 {
    text.chars().map(glyph_em).sum::<f32>() * size
}

/// Sidebar headings have one line. A title that would not fit in `budget` px gets an
/// ellipsis, cut where the budget runs out, so it follows the sidebar's width.
fn fit(text: &str, size: f32, budget: f32) -> String {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if estimate(&text, size) <= budget {
        return text;
    }
    let ellipsis = estimate("\u{2026}", size).max(size);
    let mut used = 0.0;
    let mut out = String::new();
    for c in text.chars() {
        let next = used + glyph_em(c) * size;
        if next + ellipsis > budget {
            break;
        }
        used = next;
        out.push(c);
    }
    let kept = out.trim_end().len();
    out.truncate(kept);
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::{PAGE_COLUMN, estimate, fit, label_budget};

    const LONG: &str = "A very long chapter title that cannot fit in the sidebar at all";

    #[test]
    fn short_titles_are_left_alone() {
        assert_eq!(fit("Short", 13.0, label_budget(320.0, 14.0)), "Short");
    }

    #[test]
    fn the_cut_follows_the_sidebar_width() {
        let narrow = fit(LONG, 13.0, label_budget(260.0, 14.0));
        let wide = fit(LONG, 13.0, label_budget(480.0, 14.0));
        assert!(narrow.ends_with('\u{2026}'));
        assert!(wide.chars().count() > narrow.chars().count());
        assert!(
            fit(LONG, 13.0, label_budget(2000.0, 14.0)) == LONG,
            "a very wide sidebar shows it whole"
        );
    }

    #[test]
    fn deeper_headings_get_less_room() {
        let top = fit(LONG, 12.0, label_budget(320.0, 14.0));
        let deep = fit(LONG, 12.0, label_budget(320.0, 14.0 + 3.0 * 14.0));
        assert!(deep.chars().count() < top.chars().count());
    }

    #[test]
    fn a_title_never_exceeds_its_budget() {
        let titles = [
            LONG,
            "WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW",
            "iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii",
            "第一章 很长很长很长很长很长很长很长很长的标题",
            "Tab\tand\nnewline in a heading that goes on and on",
        ];
        for width in [120.0, 200.0, 260.0, 320.0, 480.0, 800.0] {
            for indent in [14.0, 28.0, 56.0] {
                let budget = label_budget(width, indent);
                for title in titles {
                    let cut = fit(title, 13.0, budget);
                    let fits = estimate(&cut, 13.0) <= budget;
                    // Below one ellipsis wide, the ellipsis alone is all that can be shown.
                    assert!(fits || cut == "\u{2026}", "{title:?} at {width}: {cut:?}");
                }
            }
        }
    }

    #[test]
    fn a_collapsed_sidebar_does_not_panic() {
        assert_eq!(label_budget(0.0, 56.0), 0.0);
        assert_eq!(fit(LONG, 13.0, 0.0), "\u{2026}");
        const { assert!(PAGE_COLUMN > 0.0) };
    }
}
