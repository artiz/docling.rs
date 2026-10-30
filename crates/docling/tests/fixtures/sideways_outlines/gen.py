import pymupdf
question = ("Can the inspection reports of the pump stations be exported as a spreadsheet, "
            "and is it possible to filter them by district and by month of the inspection? ") * 2
answer = ("Yes. Every report can be exported, and the export keeps the district, the station "
          "number and the date of the inspection so the list can be filtered later. ") * 2
table = pymupdf.open()
page = table.new_page(width=842, height=595)
cols, rows = [40, 90, 470, 802], [40, 70, 300, 555]
for x in cols: page.draw_line((x, rows[0]), (x, rows[-1]))
for y in rows: page.draw_line((cols[0], y), (cols[-1], y))
cells = [["No.", "Question", "Answer"], ["1", question, answer], ["2", question, answer]]
for (y0, y1), row in zip(zip(rows, rows[1:]), cells):
    for (x0, x1), text in zip(zip(cols, cols[1:]), row):
        page.insert_textbox(pymupdf.Rect(x0 + 4, y0 + 6, x1 - 4, y1 - 4), text, fontsize=11)
doc = pymupdf.open()
doc.new_page(width=595, height=842).show_pdf_page(pymupdf.Rect(0, 0, 595, 842), table, 0, rotate=90)
doc.save("body.pdf")
