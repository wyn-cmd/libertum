//! Bookmarks inspector: every named anchor, editable in place, with the page it points at.
//! A bookmark is a point, so its name is what the reader navigates by.

use iced::widget::{button, column, container, row, space, text, text_input};
use iced::{Element, Length, Padding};

use crate::app::Message;
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::divider;

use libertum_model::BlockId;

/// One row in the Bookmarks pane. `page` is absent until the paragraph has been paginated.
#[derive(Clone, Debug, PartialEq)]
pub struct BookmarkEntry {
    pub block: BlockId,
    pub index: usize,
    pub offset: usize,
    pub name: String,
    pub page: Option<u32>,
}

pub fn header<'a>(t: Tokens) -> Element<'a, Message> {
    column![
        container(text(s::PANE_BOOKMARKS).size(15.0).color(t.text)).padding(Padding {
            top: 14.0,
            bottom: 12.0,
            left: 14.0,
            right: 14.0,
        }),
        divider(t),
    ]
    .into()
}

/// `draft` is the name being typed for one row; it is committed on Return so the model never
/// sees a half-typed, repaired name.
pub fn body<'a>(
    bookmarks: &'a [BookmarkEntry],
    draft: Option<(BlockId, usize, &'a str)>,
    t: Tokens,
) -> Element<'a, Message> {
    let add = container(
        button(text(s::BOOKMARK_ADD).size(12.0).color(t.accent))
            .padding(Padding {
                top: 5.0,
                bottom: 5.0,
                left: 14.0,
                right: 14.0,
            })
            .style(button::text)
            .on_press(Message::AddBookmark),
    )
    .width(Length::Fill);
    if bookmarks.is_empty() {
        return column![
            container(text(s::BOOKMARKS_EMPTY).size(12.0).color(t.text_secondary)).padding(
                Padding {
                    top: 12.0,
                    bottom: 6.0,
                    left: 14.0,
                    right: 14.0,
                }
            ),
            add,
        ]
        .into();
    }
    let mut list = column![].spacing(8.0);
    for bookmark in bookmarks {
        let name = match draft {
            Some((block, index, text)) if block == bookmark.block && index == bookmark.index => {
                text
            }
            _ => bookmark.name.as_str(),
        };
        let page = text(
            bookmark
                .page
                .map(|page| page.to_string())
                .unwrap_or_default(),
        )
        .size(11.0)
        .color(t.text_secondary);
        let remove = button(text(s::BOOKMARK_REMOVE).size(10.0).color(t.text_secondary))
            .padding(0)
            .style(button::text)
            .on_press(Message::RemoveBookmark(bookmark.block, bookmark.index));
        let reference = |label: &'static str, kind: libertum_model::CrossRefKind| {
            button(text(label).size(11.0).color(t.accent))
                .padding(0)
                .style(button::text)
                .on_press(Message::InsertCrossRef(bookmark.name.clone(), kind))
        };
        list = list.push(
            column![
                row![
                    button(text(&bookmark.name).size(12.0).color(t.text))
                        .padding(0)
                        .style(button::text)
                        .on_press(Message::GoToBookmark(bookmark.block, bookmark.index)),
                    space().width(Length::Fill),
                    page,
                    remove,
                ]
                .spacing(8.0)
                .align_y(iced::Center),
                text_input(s::BOOKMARK_NAME, name)
                    .on_input(move |value| {
                        Message::BookmarkName(bookmark.block, bookmark.index, value)
                    })
                    .on_submit(Message::BookmarkNameCommit(bookmark.block, bookmark.index))
                    .size(12)
                    .padding(6)
                    .width(Length::Fill),
                reference(s::CROSS_REF_TEXT, libertum_model::CrossRefKind::Text),
                reference(s::CROSS_REF_PAGE, libertum_model::CrossRefKind::Page),
            ]
            .spacing(3),
        );
    }
    column![
        container(text(s::BOOKMARKS_HINT).size(11.0).color(t.text_secondary)).padding(Padding {
            top: 10.0,
            bottom: 4.0,
            left: 14.0,
            right: 14.0,
        }),
        add,
        container(
            button(text(s::CROSS_REF_UPDATE).size(12.0).color(t.accent))
                .padding(Padding {
                    top: 5.0,
                    bottom: 5.0,
                    left: 14.0,
                    right: 14.0,
                })
                .style(button::text)
                .on_press(Message::RefreshCrossRefs),
        )
        .width(Length::Fill),
        container(text(s::CROSS_REF_HINT).size(11.0).color(t.text_secondary)).padding(Padding {
            top: 2.0,
            bottom: 6.0,
            left: 14.0,
            right: 14.0,
        }),
        container(list).padding(Padding {
            top: 4.0,
            bottom: 12.0,
            left: 14.0,
            right: 14.0,
        }),
    ]
    .into()
}
