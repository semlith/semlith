"""Generate test-fixture documents with real document libraries.

Run: python3 tests/fixtures/generate.py
Outputs land next to this file. Each fixture carries a unique phrase the
Rust extraction tests grep for; phrases must not repeat across fixtures.
"""

import json
import mailbox
from datetime import datetime, timezone
from email.message import EmailMessage
from email.utils import format_datetime
from pathlib import Path

import docx
import openpyxl
import pptx
from ebooklib import epub
from odf.draw import Frame, Page, TextBox
from odf.opendocument import (
    OpenDocumentPresentation,
    OpenDocumentSpreadsheet,
    OpenDocumentText,
)
from odf.style import (
    DrawingPageProperties,
    MasterPage,
    PageLayout,
    PageLayoutProperties,
    Style,
)
from odf.table import Table, TableCell, TableRow
from odf.text import P

OUT = Path(__file__).parent


def make_docx():
    d = docx.Document()
    d.add_heading("Field Report", level=1)
    d.add_paragraph(
        "The team spent the morning walking the perimeter and noting which "
        "readings had drifted since the last visit."
    )
    d.add_paragraph(
        "Most of the afternoon went to quokka thermostat calibration, which "
        "took longer than planned but finished before the shift ended."
    )
    d.add_paragraph(
        "Remaining work is minor and can be folded into next week's rotation."
    )
    t = d.add_table(rows=2, cols=2)
    t.cell(0, 0).text = "Item"
    t.cell(0, 1).text = "Status"
    t.cell(1, 0).text = "ferret ledger entry"
    t.cell(1, 1).text = "Open"
    d.save(OUT / "notes.docx")


def make_pptx():
    p = pptx.Presentation()
    layout = p.slide_layouts[1]
    for i in range(1, 13):
        s = p.slides.add_slide(layout)
        s.shapes.title.text = f"Section {i}"
        body = s.placeholders[1].text_frame
        if i == 1:
            body.text = "opening remarks placeholder"
        elif i == 11:
            body.text = "marmoset budget review"
        else:
            body.text = f"Point {i}a covering the usual ground"
        body.add_paragraph().text = f"Follow-up item for section {i}"
    p.save(OUT / "deck.pptx")


def make_xlsx():
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Summary"
    for row in [
        ["Region", "Units", "Revenue"],
        ["North", 128, 40960],
        ["South", 94, 30080],
        ["West", 211, 67520],
    ]:
        ws.append(row)
    q3 = wb.create_sheet("Q3 Notes")
    q3.append(["Note", "pangolin invoice discrepancy"])
    q3.append(["Amount", 1450.75])
    q3.append(["Days open", 12])
    wb.save(OUT / "sheet.xlsx")


def make_odt():
    d = OpenDocumentText()
    for text in [
        "Onboarding ran on schedule for the second quarter in a row.",
        "The tapir onboarding checklist was signed off by both reviewers.",
        "Nothing else is outstanding ahead of the quarterly close.",
    ]:
        d.text.addElement(P(text=text))
    d.save(OUT / "notes.odt", False)


def make_odp():
    d = OpenDocumentPresentation()
    layout = PageLayout(name="PL")
    layout.addElement(PageLayoutProperties(margin="0cm", pagewidth="28cm", pageheight="21cm"))
    d.automaticstyles.addElement(layout)
    master = MasterPage(name="Standard", pagelayoutname=layout)
    d.masterstyles.addElement(master)
    dp = Style(name="dp1", family="drawing-page")
    dp.addElement(DrawingPageProperties(backgroundsize="border"))
    d.automaticstyles.addElement(dp)

    for title, body in [
        ("Kickoff", "Scope agreed with both teams"),
        ("Timeline", "okapi rollout plan"),
        ("Risks", "Two dependencies still unconfirmed"),
    ]:
        page = Page(masterpagename=master, stylename=dp)
        d.presentation.addElement(page)
        for text, y in [(title, "2cm"), (body, "6cm")]:
            frame = Frame(width="24cm", height="3cm", x="2cm", y=y)
            page.addElement(frame)
            box = TextBox()
            frame.addElement(box)
            box.addElement(P(text=text))
    d.save(OUT / "deck.odp", False)


def make_ods():
    d = OpenDocumentSpreadsheet()
    table = Table(name="Expenses")
    for row in [
        ["Category", "Amount"],
        ["Travel", 820.5],
        ["civet expense summary", 1195.0],
        ["Supplies", 340.25],
    ]:
        tr = TableRow()
        table.addElement(tr)
        for value in row:
            if isinstance(value, str):
                cell = TableCell(valuetype="string")
            else:
                cell = TableCell(valuetype="float", value=value)
            cell.addElement(P(text=str(value)))
            tr.addElement(cell)
    d.spreadsheet.addElement(table)
    d.save(OUT / "sheet.ods", False)


def make_ipynb():
    nb = {
        "cells": [
            {
                "cell_type": "markdown",
                "metadata": {},
                "source": [
                    "# Sensor analysis\n",
                    "\n",
                    "This is the capybara regression writeup for the June batch.\n",
                ],
            },
            {
                "cell_type": "code",
                "execution_count": 1,
                "metadata": {"tags": ["calibration"]},
                "outputs": [
                    {
                        "name": "stdout",
                        "output_type": "stream",
                        "text": ["narwhal fit converged after 14 iterations\n"],
                    }
                ],
                "source": [
                    "def calibrate_axolotl(readings):\n",
                    "    # drop the warm-up samples before fitting\n",
                    "    trimmed = readings[5:]\n",
                    "    return sum(trimmed) / len(trimmed)\n",
                    "\n",
                    "print('narwhal fit converged after 14 iterations')\n",
                ],
            },
            {
                "cell_type": "markdown",
                "metadata": {},
                "source": [
                    "## Appendix\n",
                    "\n",
                    "See the quoll appendix notes for the raw sensor dumps.\n",
                ],
            },
        ],
        "metadata": {
            "kernelspec": {
                "display_name": "Python 3",
                "language": "python",
                "name": "python3",
            },
            "language_info": {
                "file_extension": ".py",
                "mimetype": "text/x-python",
                "name": "python",
                "nbconvert_exporter": "python",
                "pygments_lexer": "ipython3",
                "version": "3.11.4",
            },
        },
        "nbformat": 4,
        "nbformat_minor": 5,
    }
    (OUT / "analysis.ipynb").write_text(json.dumps(nb, indent=1) + "\n")


def make_epub():
    b = epub.EpubBook()
    b.set_identifier("urn:uuid:6f3d1b02-8c41-4f7e-9a55-2d0b7c1e44aa")
    b.set_title("The Numbat Lighthouse")
    b.set_language("en")
    b.add_author("Perpetua Vance")

    # Filenames sort zulu > mike > alpha, so a reader that ignores the spine
    # and sorts by filename emits the chapters in the wrong order.
    chapters = []
    for filename, title, body in [
        ("zulu.xhtml", "Chapter One", "The numbat lighthouse inventory was copied out twice before anyone trusted it."),
        ("alpha.xhtml", "Chapter Two", "By then the saiga ferry timetable had been pinned to the wall for a season."),
        ("mike.xhtml", "Chapter Three", "What remained was the lemur harbour survey, unfinished and still unread."),
    ]:
        c = epub.EpubHtml(title=title, file_name=filename, lang="en")
        c.content = f"<html><body><h1>{title}</h1><p>{body}</p></body></html>"
        b.add_item(c)
        chapters.append(c)

    b.toc = tuple(chapters)
    b.add_item(epub.EpubNcx())
    b.add_item(epub.EpubNav())
    b.spine = ["nav", *chapters]
    epub.write_epub(OUT / "book.epub", b)


# Literal RTF: no LibreOffice (`soffice`) and no `pandoc` on this machine, and
# PyRTF3 writes non-ASCII as raw UTF-8 rather than the \'hh and \uN escapes this
# fixture exists to exercise. RTF is plain text a person legitimately hand-writes
# -- unlike the zipped XML formats this file's docstring warns about -- so the
# literal below is the honest source. \'e9 decodes to U+00E9 and 舒? to
# U+2014 with the '?' fallback discarded.
RTF = (
    r"""{\rtf1\ansi\ansicpg1252\deff0\deflang1033
{\fonttbl{\f0\froman\fcharset0 Times New Roman;}{\f1\fswiss\fcharset0 Helvetica;}}
{\colortbl;\red0\green0\blue0;\red192\green32\blue32;\red32\green64\blue160;}
\pard\f0\fs24 The binturong pier maintenance log was reopened after the winter inspection.\par
\pard\f0\fs24 Costs rose in the caf\'e9 wing """
    # Spelled with an explicit backslash: Python decodes \uXXXX even in raw
    # strings, so the RTF escape cannot be written literally above.
    "\\u8212?"
    r""" modestly, but they rose.\par
\pard\f0\fs24 The surveyor marked the decking as \b urgent\b0  and the railings as \i deferred\i0 .\par
\pard\f0\fs24 Nothing else needs a decision before the spring review.\par
}
"""
)


def make_rtf():
    (OUT / "notes.rtf").write_text(RTF, encoding="ascii")


def make_eml():
    m = EmailMessage()
    m["From"] = "Ines Okonkwo <ines@example.org>"
    m["To"] = "Harbour Office <harbour@example.net>"
    m["Cc"] = "Records <records@example.net>, Dilip Rao <dilip@example.org>"
    m["Date"] = format_datetime(datetime(2024, 6, 11, 9, 32, tzinfo=timezone.utc))
    m["Subject"] = (
        "Résumé of the quarterly walkthrough and the follow-up items "
        "we agreed on site"
    )
    # Headers the reader is meant to drop.
    m["X-Mailer"] = "Fixture Generator 1.0"
    m["Message-ID"] = "<serval-2024-06-11-0932@example.org>"

    m.set_content(
        "The serval dispatch confirmation arrived before the café closed.\n"
        "Two crates are still unaccounted for and the ferry leaves at six.\n",
        cte="quoted-printable",
    )
    m.add_alternative(
        "<html><body><p>This html alternative must never be the extracted text."
        "</p></body></html>",
        subtype="html",
        cte="base64",
    )
    m.add_attachment(
        "crate 41: sealed\ncrate 42: missing\n",
        subtype="plain",
        filename="manifest.txt",
    )
    (OUT / "message.eml").write_bytes(m.as_bytes())


def make_mbox():
    path = OUT / "archive.mbox"
    path.unlink(missing_ok=True)
    box = mailbox.mbox(path)

    m1 = EmailMessage()
    m1["From"] = "Depot <depot@example.net>"
    m1["To"] = "ines@example.org"
    m1["Date"] = format_datetime(datetime(2024, 3, 2, 7, 15, tzinfo=timezone.utc))
    m1["Subject"] = "North route loading"
    m1.set_content("The aardwolf shipping manifest lists eleven pallets, not nine.\n")

    m2 = EmailMessage()
    m2["From"] = "Adaeze Nwosu <adaeze@example.org>"
    m2["To"] = "depot@example.net"
    m2["Date"] = format_datetime(datetime(2024, 3, 5, 16, 40, tzinfo=timezone.utc))
    m2["Subject"] = "Inventaire de l'entrepôt"
    m2.set_content(
        "The kinkajou warehouse audit closed with two open findings.\n",
        cte="quoted-printable",
    )
    m2.add_alternative(
        "<html><body><p>Ignore this markup branch entirely.</p></body></html>",
        subtype="html",
    )

    m3 = EmailMessage()
    m3["From"] = "Freight Desk <freight@example.net>"
    m3["To"] = "records@example.net"
    m3["Date"] = format_datetime(datetime(2024, 3, 9, 11, 5, tzinfo=timezone.utc))
    m3["Subject"] = "Receipt for the March haul"
    m3.set_content("Filed the vicuna freight receipt against the wrong quarter.\n")

    for m in (m1, m2, m3):
        box.add(m)
    box.flush()
    box.close()


if __name__ == "__main__":
    for fn in (
        make_docx,
        make_pptx,
        make_xlsx,
        make_odt,
        make_odp,
        make_ods,
        make_ipynb,
        make_epub,
        make_rtf,
        make_eml,
        make_mbox,
    ):
        fn()
        print(fn.__name__.removeprefix("make_"), "ok")
