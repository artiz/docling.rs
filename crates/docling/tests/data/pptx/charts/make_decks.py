"""Build the combination-chart decks of #756 the way upstream's tests do.

The decks match tests/test_backend_pptx_charts.py of docling 2.140.0
(docling#4773): `_save_combo_chart_deck` is copied from there verbatim, and
the serial-category decks follow `test_text_axis_categories_follow_the_shown_
number_format`. Run with python-pptx 1.0.2 from this directory; then write
each `<name>.{md,json}` with docling 2.140.0's PowerPoint backend (see
tests/pptx_charts.rs).
"""

import datetime
from pathlib import Path

_MONTHS = [datetime.date(2012, month, 1) for month in range(1, 13)]
_BARS = {
    "East": (85, 34, 100, 80, 116, 88, 71, 90, 121, 110, 101, 81),
    "West": (94, 72, 62, 91, 99, 70, 71, 46, 91, 57, 32, 157),
    "North": (91, 59, 85, 67, 130, 66, 51, 40, 62, 61, 61, 52),
    "South": (71, 110, 101, 28, 39, 52, 48, 125, 43, 122, 60, 63),
}
_LINES = {
    "Margin": tuple(
        n / 1000 for n in (126, 74, 110, 92, 86, 89, 77, 40, 101, 62, 81, 62)
    ),
    "Target": tuple(
        n / 1000 for n in (91, 75, 94, 72, 103, 75, 66, 81, 85, 94, 69, 95)
    ),
}


def _save_combo_chart_deck(
    path: Path, second_plot: str = "lineChart", linked_axis_title: bool = False
) -> None:
    """Save a deck whose chart draws columns and a second plot on a second axis.

    python-pptx writes single-plot charts only, so the chart starts as clustered
    columns over date categories. The last two series then move into a
    ``c:<second_plot>`` element on a secondary value axis, which is how
    PowerPoint stores a "Clustered Column - Line on Secondary Axis" chart. With
    ``second_plot="stockChart"`` the second plot is a stock plot, as in a
    "Volume-High-Low-Close" chart, for which python-pptx has no plot class.

    The second plot's category axis is hidden. It is a text axis with its own
    ``General`` number format, so it shows serial numbers, not dates. The
    category axis is titled "Region Position" and the value axes "Bar Sales".
    With ``linked_axis_title`` the category axis title is linked to a cell, so
    its text is in a cell cache instead of in rich text.
    """
    import copy

    from lxml import etree
    from pptx import Presentation
    from pptx.chart.data import CategoryChartData
    from pptx.enum.chart import XL_CHART_TYPE
    from pptx.oxml.ns import qn
    from pptx.util import Inches

    chart_data = CategoryChartData()
    chart_data.categories = _MONTHS
    for name, values in {**_BARS, **_LINES}.items():
        chart_data.add_series(name, values)
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    chart = slide.shapes.add_chart(
        XL_CHART_TYPE.COLUMN_CLUSTERED,
        Inches(0.5),
        Inches(0.5),
        Inches(9),
        Inches(6),
        chart_data,
    ).chart
    chart.category_axis.has_title = True
    chart.category_axis.axis_title.text_frame.text = "Region Position"
    chart.value_axis.has_title = True
    chart.value_axis.axis_title.text_frame.text = "Bar Sales"

    plot_area = chart.element.chart.plotArea
    if linked_axis_title:
        title_text = plot_area.find(qn("c:dateAx")).find(qn("c:title")).find(qn("c:tx"))
        title_text.remove(title_text.find(qn("c:rich")))
        str_ref = etree.SubElement(title_text, qn("c:strRef"))
        etree.SubElement(str_ref, qn("c:f")).text = "Sheet1!$A$1"
        str_cache = etree.SubElement(str_ref, qn("c:strCache"))
        etree.SubElement(str_cache, qn("c:ptCount")).set("val", "1")
        point = etree.SubElement(str_cache, qn("c:pt"))
        point.set("idx", "0")
        etree.SubElement(point, qn("c:v")).text = "Region Position"
    bar_chart = plot_area.find(qn("c:barChart"))
    second = etree.Element(qn(f"c:{second_plot}"))
    if second_plot == "lineChart":
        etree.SubElement(second, qn("c:grouping")).set("val", "standard")
        etree.SubElement(second, qn("c:varyColors")).set("val", "0")
    for ser in bar_chart.findall(qn("c:ser"))[len(_BARS) :]:
        for invert in ser.findall(qn("c:invertIfNegative")):
            ser.remove(invert)
        second.append(ser)
    if second_plot == "lineChart":
        etree.SubElement(second, qn("c:marker")).set("val", "1")
    else:
        etree.SubElement(second, qn("c:hiLowLines"))
    etree.SubElement(second, qn("c:axId")).set("val", "900")
    etree.SubElement(second, qn("c:axId")).set("val", "901")
    bar_chart.addnext(second)

    value_axis = plot_area.find(qn("c:valAx"))
    secondary_category_axis = etree.Element(qn("c:catAx"))
    etree.SubElement(secondary_category_axis, qn("c:axId")).set("val", "900")
    scaling = etree.SubElement(secondary_category_axis, qn("c:scaling"))
    etree.SubElement(scaling, qn("c:orientation")).set("val", "minMax")
    etree.SubElement(secondary_category_axis, qn("c:delete")).set("val", "1")
    etree.SubElement(secondary_category_axis, qn("c:axPos")).set("val", "b")
    number_format = etree.SubElement(secondary_category_axis, qn("c:numFmt"))
    number_format.set("formatCode", "General")
    number_format.set("sourceLinked", "0")
    etree.SubElement(secondary_category_axis, qn("c:crossAx")).set("val", "901")
    secondary_value_axis = copy.deepcopy(value_axis)
    secondary_value_axis.find(qn("c:axId")).set("val", "901")
    secondary_value_axis.find(qn("c:crossAx")).set("val", "900")
    value_axis.addnext(secondary_value_axis)
    secondary_value_axis.addnext(secondary_category_axis)
    prs.save(path)



def serial_categories(name, axis_format, source_linked, cache_format):
    from lxml import etree
    from pptx import Presentation
    from pptx.chart.data import CategoryChartData
    from pptx.enum.chart import XL_CHART_TYPE
    from pptx.oxml.ns import qn
    from pptx.util import Inches

    chart_data = CategoryChartData()
    chart_data.categories = [40909, 40940]
    chart_data.categories.number_format = cache_format
    chart_data.add_series("Sales", (10, 20))
    prs = Presentation()
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    chart = slide.shapes.add_chart(
        XL_CHART_TYPE.COLUMN_CLUSTERED,
        Inches(1),
        Inches(1),
        Inches(6),
        Inches(4),
        chart_data,
    ).chart
    if axis_format is not None and source_linked is not None:
        number_format = etree.Element(qn("c:numFmt"))
        number_format.set("formatCode", axis_format)
        number_format.set("sourceLinked", source_linked)
        category_axis = chart.element.chart.plotArea.find(qn("c:catAx"))
        category_axis.find(qn("c:axPos")).addnext(number_format)
    prs.save(name)


if __name__ == "__main__":
    _save_combo_chart_deck(Path("combo_line.pptx"))
    _save_combo_chart_deck(Path("combo_stock.pptx"), second_plot="stockChart")
    _save_combo_chart_deck(Path("combo_linked_title.pptx"), linked_axis_title=True)
    for name, args in {
        "serial_axis_date": ("m/d/yy", "0", "General"),
        "serial_linked_date": ("General", "1", "mmm d, yy"),
        "serial_no_axis_format": (None, None, "mmm d, yy"),
        "serial_axis_general": ("General", "0", "mmm d, yy"),
        "serial_axis_time": ("h:mm", "0", "General"),
    }.items():
        serial_categories(f"{name}.pptx", *args)
