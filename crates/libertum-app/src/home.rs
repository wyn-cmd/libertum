//! Home screen: the template chooser shown when Libertum starts without a document, laid
//! out like the Pages template chooser. Click a template to select it, double-click (or press
//! Create / Enter) to start a document from it. Open is shown but disabled until the DOCX,
//! ODT and Markdown importers land (Phases 3, 4 and 7).

use iced::widget::{button, column, container, mouse_area, row, space, text};
use iced::{Border, Center, Element, Length, Padding, Shadow, Vector, mouse};

use crate::app::Message;
use crate::strings as s;
use crate::templates::TemplateId;
use crate::theme::Tokens;
use crate::widgets::controls::{self, bold};

/// Thumbnail paper size, logical pixels (A4 proportions).
const THUMB_W: f32 = 132.0;
const THUMB_H: f32 = 187.0;
/// 1-inch margin at thumbnail scale (72 pt of a 595 pt wide page).
const THUMB_MARGIN: f32 = THUMB_W * 72.0 / 595.0;
/// Selection ring around the selected thumbnail.
const RING: f32 = 3.0;

pub fn view<'a>(
    selected: TemplateId,
    recents: &[std::path::PathBuf],
    t: Tokens,
) -> Element<'a, Message> {
    let cards = TemplateId::ALL.iter().fold(row![].spacing(32), |r, &id| {
        r.push(card(id, id == selected, t))
    });

    let header = column![
        text(s::HOME_TITLE).size(22).font(bold()).color(t.text),
        text(s::HOME_SUBTITLE).size(13).color(t.text_secondary),
    ]
    .spacing(4);

    let mut gallery = column![header, cards]
        .spacing(28)
        .padding(Padding::from([40.0, 48.0]));
    if !recents.is_empty() {
        gallery = gallery.push(recent_list(recents, t));
    }

    let open = button(text(s::HOME_OPEN).size(13))
        .padding(Padding::from([6.0, 16.0]))
        .on_press(Message::OpenFile)
        .style(move |_, status| secondary_button(t, status));

    let stitch = button(text(s::HOME_STITCH).size(13))
        .padding(Padding::from([6.0, 16.0]))
        .on_press(Message::OpenStitch)
        .style(move |_, status| secondary_button(t, status));

    let library = button(text(s::HOME_LIBRARY).size(13))
        .padding(Padding::from([6.0, 16.0]))
        .on_press(Message::OpenLibrary)
        .style(move |_, status| secondary_button(t, status));

    let create = button(text(s::HOME_CREATE).size(13).font(bold()))
        .padding(Padding::from([6.0, 20.0]))
        .on_press(Message::CreateDocument(selected))
        .style(move |_, status| primary_button(t, status));

    let footer = container(
        row![space().width(Length::Fill), library, stitch, open, create]
            .spacing(10)
            .align_y(Center),
    )
    .padding(Padding::from([12.0, 20.0]))
    .style(move |_| container::background(t.chrome));

    container(column![
        container(gallery).width(Length::Fill).height(Length::Fill),
        controls::hairline(t.chrome_border),
        footer,
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container::background(t.canvas))
    .into()
}

/// Recently opened files, newest first: the file name with its folder underneath.
fn recent_list<'a>(recents: &[std::path::PathBuf], t: Tokens) -> Element<'a, Message> {
    let rows = recents.iter().fold(column![].spacing(2), |list, path| {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let folder = path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_default();
        let message = Message::OpenRecent(path.clone());
        list.push(
            button(
                column![
                    text(name).size(13).color(t.text),
                    text(folder).size(11).color(t.text_secondary),
                ]
                .spacing(1),
            )
            .padding(Padding::from([6.0, 10.0]))
            .width(Length::Fill)
            .style(move |_, status| secondary_button(t, status))
            .on_press(message),
        )
    });
    column![
        text(s::HOME_RECENT).size(13).font(bold()).color(t.text),
        container(rows).width(Length::Fixed(360.0)),
    ]
    .spacing(8)
    .into()
}

fn card<'a>(id: TemplateId, selected: bool, t: Tokens) -> Element<'a, Message> {
    let ring = container(thumbnail(id, t))
        .padding(RING + 2.0)
        .style(move |_| container::Style {
            border: Border {
                color: if selected { t.accent } else { t.canvas },
                width: RING,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        });
    let name =
        text(id.name())
            .size(13)
            .font(bold())
            .color(if selected { t.accent } else { t.text });
    let description = text(id.description())
        .size(11)
        .color(t.text_secondary)
        .width(THUMB_W + 2.0 * (RING + 2.0))
        .align_x(Center);
    mouse_area(column![ring, name, description].spacing(6).align_x(Center))
        .on_press(Message::SelectTemplate(id))
        .on_double_click(Message::CreateDocument(id))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

/// A miniature of the first page the template produces. Lines stand in for text.
fn thumbnail<'a>(id: TemplateId, t: Tokens) -> Element<'a, Message> {
    let content: Element<'a, Message> = match id {
        TemplateId::Blank => space().into(),
        TemplateId::Book => {
            // The title page: title and author centred a third of the way down.
            let body_w = THUMB_W - 2.0 * THUMB_MARGIN;
            column![
                space().height((THUMB_H - 2.0 * THUMB_MARGIN) * 0.3),
                line(body_w * 0.6, 5.0, t.paper_ink_strong),
                line(body_w * 0.35, 2.0, t.paper_ink),
            ]
            .spacing(8)
            .width(Length::Fill)
            .align_x(Center)
            .into()
        }
        TemplateId::Manuscript => {
            let body_w = THUMB_W - 2.0 * THUMB_MARGIN;
            // Double spacing: a wide gap between thin lines.
            let mut lines = column![
                line(body_w * 0.55, 3.0, t.paper_ink_strong),
                line(body_w * 0.4, 2.0, t.paper_ink),
                space().height(6.0),
                line(body_w * 0.35, 3.0, t.paper_ink_strong),
            ]
            .spacing(6);
            for i in 0..9 {
                let w = if i == 8 { body_w * 0.6 } else { body_w };
                lines = lines.push(line(w, 2.0, t.paper_ink));
            }
            lines.into()
        }
    };
    container(content)
        .width(THUMB_W)
        .height(THUMB_H)
        .padding(THUMB_MARGIN)
        .style(move |_| container::Style {
            background: Some(t.paper.into()),
            border: Border {
                radius: 2.0.into(),
                ..Border::default()
            },
            shadow: Shadow {
                color: t.shadow,
                offset: Vector::new(0.0, 1.0),
                blur_radius: 4.0,
            },
            ..container::Style::default()
        })
        .into()
}

fn line<'a>(width: f32, height: f32, colour: iced::Color) -> Element<'a, Message> {
    container(space())
        .width(width)
        .height(height)
        .style(move |_| container::background(colour))
        .into()
}

fn primary_button(t: Tokens, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => t.accent.scale_alpha(0.9),
        button::Status::Pressed => t.accent.scale_alpha(0.8),
        _ => t.accent,
    };
    button::Style {
        background: Some(background.into()),
        text_color: t.on_accent,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

fn secondary_button(t: Tokens, status: button::Status) -> button::Style {
    let disabled = status == button::Status::Disabled;
    button::Style {
        background: Some(
            if status == button::Status::Hovered {
                t.control_hover
            } else {
                t.control
            }
            .into(),
        ),
        text_color: if disabled { t.text_disabled } else { t.text },
        border: Border {
            color: t.control_border,
            width: 1.0,
            radius: 6.0.into(),
        },
        ..button::Style::default()
    }
}
