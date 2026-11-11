//! Headers inspector: every heading in the document, indented by outline level, with the
//! page it starts on. Clicking a row moves the caret there.

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

pub fn body<'a>(headings: &'a [HeadingEntry], t: Tokens) -> Element<'a, Message> {
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
    for heading in headings {
        let level = heading.level.max(1);
        let indent = 14.0 + f32::from(level.saturating_sub(1)) * 14.0;
        let label = text(&heading.text)
            .size(if level == 1 { 13.0 } else { 12.0 })
            .color(if level == 1 { t.text } else { t.text_secondary });
        let page = text(
            heading
                .page
                .map(|page| page.to_string())
                .unwrap_or_default(),
        )
        .size(11.0)
        .color(t.text_secondary);
        list = list.push(
            button(
                row![label, space().width(Length::Fill), page]
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
