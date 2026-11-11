"""Generates tests/fixtures/odt/rich.odt with odfpy.

The fixture is deliberately written by a different implementation (odfpy follows the ODF 1.2
schema) so libertum-odt's importer is tested against foreign XML rather than only against its
own writer. Regenerate it with:

    py -3 tools/make_odt_fixture.py

It needs odfpy (`py -3 -m pip install odfpy`).
"""

from pathlib import Path

from odf.element import Element
from odf.namespaces import STYLENS
from odf.opendocument import OpenDocumentText
from odf.style import (
    DefaultStyle,
    Footer,
    FooterStyle,
    Header,
    HeaderFooterProperties,
    HeaderStyle,
    ListLevelProperties,
    MasterPage,
    PageLayout,
    PageLayoutProperties,
    ParagraphProperties,
    Style,
    TableCellProperties,
    TableColumnProperties,
    TableProperties,
    TextProperties,
)
from odf.table import (
    Table,
    TableCell,
    TableColumn,
    TableHeaderRows,
    TableRow,
)
from odf.text import (
    H,
    List,
    ListItem,
    ListLevelStyleBullet,
    ListStyle,
    P,
    PageCount,
    PageNumber,
    Span,
)

TARGET = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "odt" / "rich.odt"


def add_paragraph_style(doc, name, display, parent="Standard", **props):
    style = Style(
        name=name,
        displayname=display,
        family="paragraph",
        parentstylename=parent,
    )
    if props:
        style.addElement(ParagraphProperties(**props))
    doc.styles.addElement(style)
    return style


def main():
    doc = OpenDocumentText()

    # Document defaults.
    defaults = DefaultStyle(family="paragraph")
    defaults.addElement(ParagraphProperties(margintop="0in", marginbottom="0.08in"))
    defaults.addElement(
        TextProperties(fontfamily="Georgia", fontsize="12pt", color="#000000")
    )
    doc.styles.addElement(defaults)

    # Common paragraph styles.
    body = Style(name="Standard", displayname="Body", family="paragraph")
    doc.styles.addElement(body)

    heading = add_paragraph_style(
        doc,
        "Heading_20_1",
        "Heading 1",
        margintop="18pt",
        keepwithnext="always",
        keeptogether="always",
    )
    heading.addElement(
        TextProperties(fontsize="16pt", fontweight="bold", color="#1A1A1A")
    )

    quote = add_paragraph_style(
        doc,
        "Quote",
        "Quote",
        marginleft="0.5in",
        marginright="0.5in",
        textalign="center",
    )
    quote.addElement(TextProperties(fontstyle="italic"))

    # Automatic text styles for the spans.
    strong = Style(name="T1", family="text")
    strong.addElement(TextProperties(fontweight="bold"))
    doc.automaticstyles.addElement(strong)
    emphasis = Style(name="T2", family="text")
    emphasis.addElement(TextProperties(fontstyle="italic", color="#AA0000"))
    doc.automaticstyles.addElement(emphasis)

    # A paragraph with explicit tab stops and a border.
    boxed = add_paragraph_style(doc, "Boxed", "Boxed")
    boxed.addElement(
        ParagraphProperties(
            border="0.5pt solid #336699",
            padding="3pt",
            lineheight="150%",
        )
    )
    doc.automaticstyles.addElement(boxed)

    # Page break paragraph style.
    page_break = Style(name="PBreak", family="paragraph", parentstylename="Standard")
    page_break.addElement(ParagraphProperties(breakbefore="page"))
    doc.automaticstyles.addElement(page_break)

    # Section two starts with this automatic style (a master-page change).
    second = Style(
        name="Sect2",
        family="paragraph",
        parentstylename="Standard",
        masterpagename="Second",
    )
    doc.automaticstyles.addElement(second)

    # Page layouts and master pages.
    landscape = PageLayout(name="pm1")
    landscape.addElement(
        PageLayoutProperties(
            pagewidth="11in",
            pageheight="8.5in",
            printorientation="landscape",
            margintop="0.8in",
            marginbottom="0.8in",
            marginleft="1in",
            marginright="1in",
            firstpagenumber=3,
        )
    )
    header_style = HeaderStyle()
    header_style.addElement(HeaderFooterProperties(margintop="0.4in"))
    landscape.addElement(header_style)
    footer_style = FooterStyle()
    footer_style.addElement(HeaderFooterProperties(marginbottom="0.5in"))
    landscape.addElement(footer_style)
    doc.automaticstyles.addElement(landscape)

    portrait = PageLayout(name="pm2")
    portrait.addElement(
        PageLayoutProperties(
            pagewidth="8.5in",
            pageheight="11in",
            margintop="1in",
            marginbottom="1in",
            marginleft="1in",
            marginright="1in",
        )
    )
    doc.automaticstyles.addElement(portrait)

    standard = MasterPage(name="Standard", pagelayoutname="pm1")
    header = Header()
    header.addElement(P(text="Fixture Header"))
    standard.addElement(header)
    first_header = Element(qname=(STYLENS, "header-first"))
    first_header.addElement(P(text="First page header"))
    # odfpy's grammar predates the ODF 1.2 first-page variants, so skip its check.
    standard.addElement(first_header, check_grammar=False)
    footer = Footer()
    para = P()
    para.addText("Page ")
    para.addElement(PageNumber())
    para.addText(" of ")
    para.addElement(PageCount())
    footer.addElement(para)
    standard.addElement(footer)
    # An empty first-page footer: the page number is not drawn on page one.
    standard.addElement(
        Element(qname=(STYLENS, "footer-first")), check_grammar=False
    )
    doc.masterstyles.addElement(standard)

    doc.masterstyles.addElement(MasterPage(name="Second", pagelayoutname="pm2"))

    # List style: bullets.
    bullets = ListStyle(name="LBullet")
    for level in range(1, 4):
        bullet = ListLevelStyleBullet(
            level=level, bulletchar=["•", "◦", "▪"][level - 1]
        )
        bullet.addElement(
            ListLevelProperties(spacebefore="0.25in", minlabelwidth="0.25in")
        )
        bullets.addElement(bullet)
    doc.automaticstyles.addElement(bullets)

    # Table styles.
    table_style = Style(name="Table1", family="table")
    table_style.addElement(TableProperties(width="4.5in"))
    doc.automaticstyles.addElement(table_style)
    column_style = Style(name="TCol1", family="table-column")
    column_style.addElement(TableColumnProperties(columnwidth="2in"))
    doc.automaticstyles.addElement(column_style)
    column_style2 = Style(name="TCol2", family="table-column")
    column_style2.addElement(TableColumnProperties(columnwidth="2.5in"))
    doc.automaticstyles.addElement(column_style2)
    cell_style = Style(name="TCell1", family="table-cell")
    cell_style.addElement(
        TableCellProperties(border="1pt solid #444444", backgroundcolor="#DDEEFF")
    )
    doc.automaticstyles.addElement(cell_style)
    plain_cell = Style(name="TCell2", family="table-cell")
    plain_cell.addElement(TableCellProperties(border="1pt solid #444444"))
    doc.automaticstyles.addElement(plain_cell)

    # ------------------------------------------------------------------ body
    doc.text.addElement(
        H(outlinelevel=1, stylename=heading, text="Fixture Heading")
    )
    first = P()
    first.addText("Plain Georgia text with ")
    bold = Span(stylename=strong)
    bold.addText("bold words")
    first.addElement(bold)
    first.addText(" and ")
    italic = Span(stylename=emphasis)
    italic.addText("italic words")
    first.addElement(italic)
    first.addText(".")
    doc.text.addElement(first)

    doc.text.addElement(P(text="A quoted line.", stylename=quote))
    doc.text.addElement(P(text="A boxed paragraph with a border.", stylename=boxed))

    doc.text.addElement(P(text="Things to do:", stylename=body))
    bullet_list = List(stylename=bullets)
    for text in ["First bullet", "Second bullet"]:
        item = ListItem()
        item.addElement(P(text=text, stylename=body))
        bullet_list.addElement(item)
    doc.text.addElement(bullet_list)

    doc.text.addElement(P(text="After the page break.", stylename=page_break))

    table = Table(name="FixtureTable", stylename=table_style)
    table.addElement(TableColumn(stylename=column_style))
    table.addElement(TableColumn(stylename=column_style2))
    header_rows = TableHeaderRows()
    header_row = TableRow()
    for text in ["Head A", "Head B"]:
        cell = TableCell(stylename=cell_style, valuetype="string")
        cell.addElement(P(text=text, stylename=body))
        header_row.addElement(cell)
    header_rows.addElement(header_row)
    table.addElement(header_rows)
    data_row = TableRow()
    cell = TableCell(stylename=plain_cell, valuetype="string")
    cell.addElement(P(text="Body A", stylename=body))
    data_row.addElement(cell)
    spanned = TableCell(
        stylename=plain_cell, valuetype="string", numbercolumnsspanned=1
    )
    spanned.addElement(P(text="Body B", stylename=body))
    data_row.addElement(spanned)
    table.addElement(data_row)
    doc.text.addElement(table)

    doc.text.addElement(
        P(text="Second section starts here.", stylename=second)
    )

    TARGET.parent.mkdir(parents=True, exist_ok=True)
    doc.save(str(TARGET))
    print(f"wrote {TARGET}")


if __name__ == "__main__":
    main()
