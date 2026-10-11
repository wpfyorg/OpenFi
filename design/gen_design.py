#!/usr/bin/env python3
"""Generates design/openwrtfi.pen (pen.dev format). Re-run after edits:
    python3 design/gen_design.py
Dark, dense "looking glass" style. Sample data mirrors the real home network
(see home-router-setup/AGENTS.md): mainrouter + ap-2 (wired+mesh) + ap-4 (mesh only)."""
import itertools, json, math, os, random

_ids = itertools.count(1)
def nid(): return f"n{next(_ids):04d}"
random.seed(11)

SANS, MONO = "IBM Plex Sans", "IBM Plex Mono"
W, SIDE, PAD = 1440, 220, 32
C = W - SIDE - 2 * PAD          # content width = 1156
GAP = 14

# ------------------------------------------------------------------ primitives
def frame(name, children=(), **p):
    n = {"type": "frame", "id": nid(), "name": name, **p}
    kids = [c for c in children if c]
    if kids: n["children"] = kids
    return n

def text(content, size=13, color="$text", weight=None, font=SANS, name=None, **p):
    n = {"type": "text", "id": nid(), "name": name or str(content)[:24], "content": str(content),
         "fontFamily": font, "fontSize": size, "fill": color, **p}
    if weight: n["fontWeight"] = weight
    return n

def mono(content, size=12, color="$text", weight=None, **p):
    return text(content, size, color, weight, MONO, **p)

def label(content, color="$text-3"):
    """Small sentence-case field label (no tracked-out caps)."""
    return text(content, 11, color, "500")

def icon(name, size=15, color="$text-2"):
    return {"type": "icon", "id": nid(), "name": f"icon {name}", "library": "lucide", "icon": name,
            "width": size, "height": size, "fill": color}

def rect(name, w, h, fill, r=0, **p):
    n = {"type": "rectangle", "id": nid(), "name": name, "width": w, "height": h, "fill": fill, **p}
    if r: n["cornerRadius"] = r
    return n

def dot(color, s=7):
    return {"type": "ellipse", "id": nid(), "name": "dot", "width": s, "height": s, "fill": color}

def path(name, geometry, w, h, fill=None, stroke=None, sw=1.5, **p):
    n = {"type": "path", "id": nid(), "name": name, "geometry": geometry, "viewBox": [0, 0, w, h],
         "x": 0, "y": 0, "width": w, "height": h, **p}
    if fill: n["fill"] = fill
    if stroke: n.update(stroke=stroke, strokeWidth=sw, strokeLinejoin="round", strokeLinecap="round")
    return n

def row(name, children, **p): return frame(name, children, **{"alignItems": "center", **p})
def col(name, children, **p): return frame(name, children, layout="vertical", **p)

def panel(name, children, width="fill_container", pad=16, gap=12, **p):
    return frame(name, children, layout="vertical", width=width, fill="$panel", cornerRadius=10,
                 stroke="$border", strokeWidth=1, padding=pad, gap=gap, clip=True, **p)

def panel_head(title, sub=None, right=None):
    return row("Panel Header", [col("Titles", [text(title, 14, weight="600"),
                                               text(sub, 12, "$text-3") if sub else None], gap=3),
                                right], width="fill_container", justifyContent="space_between")

def seg(options, active, name="Segmented"):
    """Segmented control like 'All | Overlay | WAN | LAN'."""
    items = [frame(f"Opt {o}", [text(o, 12, "$text" if o == active else "$text-2", "500")], padding=[5, 10],
                   cornerRadius=5, **({"fill": "$raised"} if o == active else {})) for o in options]
    return row(name, items, padding=3, gap=2, cornerRadius=6, stroke="$border", strokeWidth=1)

def button(label_, ic=None, primary=False):
    return row(f"Btn {label_}", [icon(ic, 13, "#FFFFFF" if primary else "$text-2") if ic else None,
                                 text(label_, 12, "#FFFFFF" if primary else "$text", "500", font=MONO if "·" in label_ else SANS)],
               padding=[7, 11], gap=7, cornerRadius=6,
               **({"fill": "$section"} if primary else {"stroke": "$border", "strokeWidth": 1}))

def chip(label_, color):
    return row(f"Chip {label_}", [dot(color, 6), text(label_, 11, "$text-2", "500")], gap=5,
               padding=[2, 7], cornerRadius=5, stroke="$border", strokeWidth=1)

def share_bar(pct, w=70, color="$section"):
    return frame("Share", [rect("Fill", max(2, round(w * pct)), 4, color, 2)], width=w, height=4,
                 fill="$track", cornerRadius=2, clip=True)

def legend(items):
    return row("Legend", [row(f"L {n}", [rect("sw", 8, 8, c, 2), text(n, 11, "$text-2")], gap=6) for n, c in items],
               gap=16, justifyContent="center", width="fill_container")

# ------------------------------------------------------------------ charts
def series(n, base, amp, spike=0.04, lo=0.0, seed=None, smooth=0.6):
    r = random.Random(seed)
    v, out = base, []
    for i in range(n):
        target = base + amp * math.sin(i / n * math.pi * 2.2 + 1.2) * 0.5
        v = v * smooth + target * (1 - smooth) + r.uniform(-amp, amp) * 0.25
        x = max(lo, v)
        if r.random() < spike: x += r.uniform(1.5, 4) * amp
        out.append(x)
    return out

def area_chart(name, pw, ph, layers, ymax, yfmt, xlabels, mirror=None, stacked=False, grid=4,
               mirror_max=None, mirror_frac=0.3):
    """layers: [(values, color)] above the axis. mirror: layers drawn below it (upload), in their own
    smaller band (`mirror_frac` of the height) with their own scale (`mirror_max`)."""
    kids = []
    below = round(ph * mirror_frac) if mirror else 0
    zero = ph - below
    mmax = mirror_max or ymax
    ticks = []  # (y, label)
    for g in range(grid + 1):
        y = round(g * (zero - 1) / grid)
        kids.append(rect(f"grid {g}", pw, 1, "$grid", x=0, y=y))
        ticks.append((y, yfmt(ymax * (1 - g / grid))))
    if mirror:
        kids.append(rect("grid below", pw, 1, "$grid", x=0, y=ph - 1))
        ticks[-1] = (zero, yfmt(0))
        ticks.append((ph - 1, yfmt(mmax)))
    def poly(vals, base_vals, sign):
        span, top = (zero, ymax) if sign > 0 else (below, mmax)
        n = len(vals)
        y = lambda v: zero - sign * min(span, v / top * span)
        pts = [(i * pw / (n - 1), y(v + b)) for i, (v, b) in enumerate(zip(vals, base_vals))]
        bpts = [(i * pw / (n - 1), y(b)) for i, b in enumerate(base_vals)]
        line = "M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in pts)
        area = line + " L" + " L".join(f"{x:.1f},{y:.1f}" for x, y in reversed(bpts)) + " Z"
        return area, line
    for group, sign in ((layers, 1), (mirror or [], -1)):
        base = [0.0] * len(group[0][0]) if group else []
        for vals, color in group:
            b = base if stacked else [0.0] * len(vals)
            a, l = poly(vals, b, sign)
            kids.append(path(f"area {color}", a, pw, ph, fill=color + "24"))
            kids.append(path(f"line {color}", l, pw, ph, stroke=color, sw=1.2))
            if stacked: base = [x + y for x, y in zip(base, vals)]
    if mirror:
        kids.append(rect("zero axis", pw, 1, "$border-strong", x=0, y=zero))
    plot = frame("Plot", kids, layout="none", width=pw, height=ph, clip=True)
    # tick labels sit at their real y (the two bands have different scales)
    yaxis = frame("Y Axis", [mono(t, 10, "$text-3", textGrowth="fixed-width", width=40, textAlign="right",
                                  x=0, y=max(0, min(ph - 13, y - 6))) for y, t in ticks],
                  layout="none", width=40, height=ph)
    xaxis = row("X Axis", [mono(t, 10, "$text-3") for t in xlabels], width=pw, justifyContent="space_between")
    return col(name, [row("Chart Body", [yaxis, plot], gap=8, alignItems="start"),
                      row("X Row", [frame("pad", [], width=40, height=1), xaxis], gap=8)], gap=8)

def bar_chart(name, pw, ph, groups, ymax, yfmt, xlabels, grid=4):
    """groups: list of per-slot lists of (value, color) stacked bottom-up; slots side by side."""
    cols = []
    for gi, stack in enumerate(groups):
        segs = [rect(f"seg {c}", "fill_container", max(1, round(v / ymax * ph)), c, 2 if j == len(stack) - 1 else 0)
                for j, (v, c) in enumerate(reversed(stack))]
        cols.append(col(f"Slot {gi}", segs, width="fill_container", height=ph, justifyContent="end", gap=1))
    grid_rects = [rect(f"grid {g}", pw, 1, "$grid", x=0, y=round(g * (ph - 1) / grid)) for g in range(grid + 1)]
    bars = row("Bars", cols, width=pw, height=ph, gap=max(2, pw // len(groups) // 3), alignItems="end",
               x=0, y=0, layoutPosition="absolute")
    plot = frame("Plot", grid_rects + [bars], layout="none", width=pw, height=ph)
    ticks = [yfmt(ymax * (1 - g / grid)) for g in range(grid + 1)]
    yaxis = col("Y Axis", [mono(t, 10, "$text-3", textGrowth="fixed-width", width=40, textAlign="right") for t in ticks],
                height=ph, justifyContent="space_between")
    xaxis = row("X Axis", [mono(t, 10, "$text-3") for t in xlabels], width=pw, justifyContent="space_between")
    return col(name, [row("Chart Body", [yaxis, plot], gap=8, alignItems="start"),
                      row("X Row", [frame("pad", [], width=40, height=1), xaxis], gap=8)], gap=8)

def sparkline(name, w, h, vals, color):
    mx = max(vals) * 1.1
    pts = [(i * w / (len(vals) - 1), h - v / mx * h) for i, v in enumerate(vals)]
    line = "M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in pts)
    return frame(name, [path("area", line + f" L{w},{h} L0,{h} Z", w, h, fill=color + "30"),
                        path("line", line, w, h, stroke=color, sw=1.2)], layout="none", width=w, height=h, clip=True)

def traffic(n, peak, seed, start_hour=19, floor=0.08):
    """24h of realistic household traffic: quiet overnight, lunch bump, evening streaming peak, bursty."""
    r = random.Random(seed)
    out, v = [], 0.0
    for i in range(n):
        h = (start_hour + 24 * i / n) % 24
        evening = min(abs(h - 21.5), 24 - abs(h - 21.5))  # circular distance to the 21:30 peak
        shape = floor + 0.6 * math.exp(-evening ** 2 / 6) + 0.25 * math.exp(-(h - 13) ** 2 / 4)
        target = peak * min(1.0, shape) * math.exp(r.gauss(0, 0.35))
        v = v * 0.5 + target * 0.5
        if r.random() < 0.03: v += peak * r.uniform(0.3, 0.7)  # a download or update burst
        out.append(max(peak * 0.02, v))
    return out

mbps = lambda v: f"{v:.0f}"
gbf = lambda v: f"{v:.0f} GB"
# Colour roles. Each hue has exactly one job so colour carries meaning across every screen.
DOWN, UP = "#4EA8FF", "#F2B14C"                                             # traffic direction
NODE = {"mainrouter": "#2CC5C5", "ap-2": "#F06BA8", "ap-4": "#A48BFF"}       # node identity, everywhere
DEVICE = ["#7AA2FF", "#FF9F6B", "#E879F9", "#FACC6B", "#6B7686"]           # per-device series (last = others)
SECTION = {"Dashboard": "#4E8BFF", "Topology": "#22B8A6", "Traffic": "#2AA7E0", "Usage": "#E8A33C",
           "Devices": "#E0609A", "WiFi": "#9476F0", "DNS": "#EF5F5F", "QoS": "#F0873C", "Settings": "#3CC4D8"}
THEME = {"Dashboard": "dashboard", "WiFi": "wifi", "Devices": "devices", "Usage": "usage",
         "Topology": "topology", "Settings": "settings"}

def tile(ic, hue, size=22, solid=False, icon_size=None):
    """Coloured icon tile: tinted by default, solid for the active/current item."""
    return frame(f"Tile {ic}", [icon(ic, icon_size or round(size * 0.6), "#FFFFFF" if solid else hue)], width=size, height=size,
                 cornerRadius=round(size * 0.28), fill=hue if solid else hue + "26", justifyContent="center", alignItems="center")

def node_tag(n, size=12):
    """A node name in its identity colour, with a small swatch."""
    return row(f"NodeTag {n}", [rect("sw", 3, size, NODE.get(n, "#6B7686"), 1), mono(n, size, "$text", "500")], gap=6)

HOURS = ["19:00", "22:00", "01:00", "04:00", "07:00", "10:00", "13:00", "16:00", "Now"]

# ------------------------------------------------------------------ chrome
NAV = [("gauge", "Dashboard"), ("network", "Topology"), ("arrow-left-right", "Traffic"), ("chart-column", "Usage"),
       ("monitor-smartphone", "Devices"), ("wifi", "WiFi"), ("shield-check", "DNS"), ("sliders-horizontal", "QoS"),
       ("settings", "Settings")]

def sidebar(active):
    items = [row(f"Nav {n}", [tile(ic, SECTION[n], 22, solid=(n == active)),
                              text(n, 13, "$text" if n == active else "$text-2", "600" if n == active else "500")],
                 width="fill_container", padding=[6, 8], gap=10, cornerRadius=7,
                 **({"fill": SECTION[n] + "1A"} if n == active else {})) for ic, n in NAV]
    brand = row("Brand", [frame("Mark", [mono("fi", 13, "#FFFFFF", "700")], width=30, height=30, fill="#4E8BFF",
                                cornerRadius=7, justifyContent="center", alignItems="center"),
                          col("Name", [text("OpenWrtFi", 14, weight="700"), label("home · 3 nodes")], gap=2)],
                gap=10, padding=[2, 6])
    nodes = col("Nodes", [label("Nodes")] + [
        row(f"Node {n}", [rect("sw", 3, 12, NODE[n], 1), mono(n, 11, "$text-2"), frame("sp", [], width="fill_container", height=1),
                          dot("$ok", 6), mono(v, 11, "$text-3")], width="fill_container", gap=7)
        for n, v in [("mainrouter", "14%"), ("ap-2", "6%"), ("ap-4", "4%")]],
        gap=8, width="fill_container", padding=[12, 10], cornerRadius=8, stroke="$border", strokeWidth=1)
    foot = row("Collapse", [icon("panel-left", 15), text("Collapse", 12, "$text-2")], gap=10, padding=[8, 10])
    return col("Sidebar", [brand, col("Nav", items, gap=2, width="fill_container"),
                           frame("Spacer", [], width="fill_container", height="fill_container"), nodes, foot],
               width=SIDE, height="fill_container", fill="$sidebar", padding=[16, 12], gap=20,
               stroke="$border", strokeWidth={"right": 1})

def topbar():
    search = row("Search", [icon("search", 14, "$text-3"), text("Search devices, IPs, MACs, SSIDs…", 13, "$text-3"),
                            frame("sp", [], width="fill_container", height=1),
                            frame("Kbd", [mono("/", 11, "$text-3")], padding=[1, 6], cornerRadius=4, stroke="$border", strokeWidth=1)],
                 width=400, padding=[7, 10], gap=8, cornerRadius=7, stroke="$border", strokeWidth=1)
    right = row("Right", [row("WAN", [dot("$ok", 7), mono("WAN 192.0.2.2", 11, "$text-2")], gap=7),
                          icon("moon", 16), row("User", [icon("circle-user", 16), text("root", 13, weight="500")], gap=7)], gap=20)
    return row("Topbar", [search, right], width="fill_container", padding=[10, PAD], justifyContent="space_between",
               stroke="$border", strokeWidth={"bottom": 1})

def page_head(title, sub, right):
    return row("Page Header", [col("Titles", [text(title, 20, weight="600", letterSpacing=-0.2), text(sub, 13, "$text-2")], gap=5),
                               right], width="fill_container", justifyContent="space_between")

def range_controls(scope=True, rng="Last 24 hours"):
    return row("Controls", [seg(["All", "Overlay", "WAN", "LAN"], "Overlay") if scope else None,
                            button(rng, "calendar"), button("5s", "refresh-cw")], gap=8)

def screen(name, x, active, head, body):
    ic = dict((n, i) for i, n in NAV)[active]
    head["children"][0] = row("Title Block", [tile(ic, SECTION[active], 40, solid=True, icon_size=20), head["children"][0]], gap=14)
    main = col("Main", [head, *body], width="fill_container", padding=[24, PAD, 40, PAD], gap=GAP)
    content = col("Content", [topbar(), main], width="fill_container")
    return frame(name, [sidebar(active), content], x=x, y=0, width=W, height="fit_content(900)", fill="$bg", clip=True,
                 theme={"section": THEME[active]})

def kpi(lbl, value, sub, ic=None, hue="#8391A7", value_color="$text", extra=None):
    """One cell of a stat band (see kpi_band)."""
    return col(f"KPI {lbl}", [row("Head", [tile(ic, hue, 24) if ic else None, label(lbl, "$text-2")], gap=8),
                              extra or text(value, 24, value_color, "600", letterSpacing=-0.5),
                              text(sub, 11, "$text-3", textGrowth="fixed-width", width="fill_container") if sub else None],
               gap=8, width="fill_container", padding=[14, 18])

def kpi_band(cells):
    """Stats read as one instrument strip: a single surface split by hairlines."""
    for c in cells[1:]:
        c.update(stroke="$border", strokeWidth={"left": 1})
    return frame("KPIs", cells, width="fill_container", fill="$panel", cornerRadius=10, stroke="$border", strokeWidth=1, clip=True)

def table(name, cols, rows_, header_fill="$raised", row_h=50):
    """cols: [(title, width, align)]; rows_: list of lists of nodes."""
    def cell(w, child, align):
        return frame("Cell", [child], width=w, height="fill_container", padding=[0, 12], alignItems="center",
                     justifyContent={"right": "end", "left": "start"}[align], layout="horizontal")
    head = row("Header Row", [cell(w, label(t), a) for t, w, a in cols], width="fill_container", height=32, fill=header_fill)
    body = [frame(f"Row {i}", [cell(w, v, a) for (t, w, a), v in zip(cols, r)], width="fill_container", height=row_h,
                  stroke="$border", strokeWidth={"top": 1}) for i, r in enumerate(rows_)]
    return col(name, [head, *body], width="fill_container")

def two(a, b, a_color="$text", b_color="$text-3", a_mono=False, b_mono=True):
    return col("Two", [(mono if a_mono else text)(a, 12 if a_mono else 13, a_color, "500"),
                       (mono if b_mono else text)(b, 11, b_color)], gap=3)

def updown(d, u):
    return col("Rates", [row("d", [text("↓", 11, "$down"), mono(d, 11)], gap=5), row("u", [text("↑", 11, "$up"), mono(u, 11)], gap=5)],
               gap=3, alignItems="end")

def sig_color(dbm): return "$green" if dbm > -60 else "$yellow" if dbm > -72 else "$red"

# ------------------------------------------------------------------ data
DEVICES = [  # name, ip, conn, band, dbm, down_now, up_now, total_gb, share, tier
    ("Client 01", "192.0.2.8", "ap-2", "5 GHz", -52, "42.10", "3.40", 64.2, 0.29, "Whitelisted"),
    ("Client 02", "192.0.2.12", "mainrouter", "5 GHz", -61, "18.90", "0.40", 51.8, 0.23, "FUP 125"),
    ("Client 03", "192.0.2.13", "ap-4", "5 GHz", -48, "6.20", "0.90", 22.4, 0.10, "Whitelisted"),
    ("Client 04", "192.0.2.4", "mainrouter", "5 GHz", -44, "1.20", "0.20", 18.7, 0.084, "Whitelisted"),
    ("Client 05 wired", "192.0.2.5", "Wired", None, None, "0.80", "0.10", 14.1, 0.063, "Whitelisted"),
    ("Client 06", "192.0.2.11", "ap-2", "2.4 GHz", -67, "0.10", "2.10", 12.9, 0.058, "Uncapped"),
    ("Client 07", "192.0.2.6", "ap-4", "5 GHz", -58, "0.80", "0.10", 9.3, 0.042, "Uncapped"),
    ("Client 08", "192.0.2.10", "Wired", None, None, "0.00", "1.90", 8.8, 0.039, "Uncapped"),
    ("Client 09", "192.0.2.7", "mainrouter", "2.4 GHz", -71, "0.00", "0.00", 0.4, 0.002, "Uncapped"),
    ("192.0.2.14", "192.0.2.14", "ap-2", "2.4 GHz", -79, "0.00", "0.00", 0.2, 0.001, "Uncapped"),
]

def conn_cell(d):
    if d[2] == "Wired": return row("Conn", [dot("$text-3", 6), text("Wired, lan", 12, "$text-2")], gap=7)
    return row("Conn", [node_tag(d[2]), text(d[3], 12, "$text-2"), dot(sig_color(d[4]), 6), mono(f"{d[4]} dBm", 11, "$text-2")], gap=7)

def tier_chip(t):
    return chip(t, {"Whitelisted": "$section", "FUP 125": "$orange"}.get(t, "$text-3"))

# ------------------------------------------------------------------ Dashboard
def dashboard(x):
    kpis = kpi_band([
        kpi("Devices online", "23", "19 on WiFi, 4 wired", "monitor-smartphone", SECTION["Devices"]),
        kpi("WiFi clients", "19", "14 on 5 GHz across 3 access points", "wifi", SECTION["WiFi"]),
        kpi("Health", "", "All 3 nodes up. Mesh links at -49 and -75 dBm.", "heart", "#3DD68C",
            extra=row("V", [dot("$green", 8), text("Healthy", 22, weight="600")], gap=8)),
        kpi("This month", "412.7 GB", "371.2 GB down, 41.5 GB up", "calendar", SECTION["Usage"])])

    bw_w = C - 380 - GAP
    pw = bw_w - 32 - 48
    dl = traffic(96, 240, seed=1); ul = [v * 0.12 for v in traffic(96, 240, seed=2)]
    bandwidth = panel("Bandwidth", [
        panel_head("Internet", "Mbps, live and over the last 24 hours. Drag the chart to zoom.", row("Compare", [label("Compare with"), seg(["Off", "Yesterday", "Last week"], "Off")], gap=8)),
        row("Live Readout", [col("Download now", [label("Download now"), row("v", [text("↓", 22, "$down", "600"), mono("184.2", 32, weight="600", letterSpacing=-0.8), text("Mbps", 13, "$text-2")], gap=6, alignItems="end")], gap=4),
                             col("Upload now", [label("Upload now"), row("v", [text("↑", 22, "$up", "600"), mono("22.6", 32, weight="600", letterSpacing=-0.8), text("Mbps", 13, "$text-2")], gap=6, alignItems="end")], gap=4),
                             col("Window", [label("Last 24 hours"), mono("38.4 GB down", 13, "$text-2"), mono("4.1 GB up", 13, "$text-2")], gap=4)],
            gap=40, alignItems="end"),
        area_chart("Bandwidth Chart", pw, 200, [(dl, DOWN)], 320, mbps, HOURS, mirror=[(ul, UP)], mirror_max=60, mirror_frac=0.25),
        legend([("Download", DOWN), ("Upload, drawn below the axis", UP)]),
        col("Overview", [label("Last 7 days. Drag the window to change the range."),
                         frame("Brush", [sparkline("Overview Line", bw_w - 32, 34, series(140, 60, 40, seed=9, smooth=0.85), DOWN),
                                         rect("Window", round((bw_w - 32) * 0.14), 34, DOWN + "20", x=round((bw_w - 32) * 0.86), y=0,
                                              stroke=DOWN, strokeWidth=1)],
                               layout="none", width=bw_w - 32, height=34)], gap=8)], width=bw_w)

    talkers = panel("Top Talkers", [
        panel_head("Top talkers", "Bytes in the window, by device."),
        table("Talkers Table", [("Device", "fill_container", "left"), ("Now", 84, "right"), ("Share", 116, "right")],
              [[two(d[0], d[1]), updown(d[5], d[6]),
                col("Share", [row("s", [share_bar(d[8] / 0.3, 40, DEVICE[min(i, 4)]), mono(f"{d[8]*100:.0f}%", 11)], gap=6), mono(f"{d[7]} GB", 11, "$text-3")],
                    gap=3, alignItems="end")] for i, d in enumerate(DEVICES[:7])]),
        row("Foot", [text("See all devices", 12, "$section", "500")], width="fill_container", justifyContent="end")],
        width=380, pad=[16, 0, 12, 0])
    talkers["children"][0] = frame("hp", [talkers["children"][0]], width="fill_container", padding=[0, 16])
    talkers["children"][2]["padding"] = [0, 16]

    # mesh / nodes
    def node_row(n, role, cpu, ram, clients, link, lcol, temp):
        hot = temp >= 70
        meter = lambda lbl_, v, c: col(lbl_, [row("h", [label(lbl_), mono(v, 10, "$text-2")], width="fill_container", justifyContent="space_between"),
                                              share_bar(int(v[:-1]) / 100, 64, c)], gap=4, width=64)
        return frame(f"Node {n}", [
            col("Who", [row("n", [rect("sw", 3, 14, NODE[n], 1), mono(n, 13, weight="600"), text(role, 11, "$text-3")], gap=8),
                        row("Link", [dot(lcol, 6), mono(link, 11, "$text-2")], gap=6)], gap=4, width="fill_container"),
            mono(f"{clients}", 12, "$text-2", textGrowth="fixed-width", width=24, textAlign="right"),
            row("Temp", [icon("thermometer", 12, "$orange" if hot else "$text-3"), mono(f"{temp}°", 11, "$orange" if hot else "$text-2")], gap=3, width=42),
            meter("CPU", cpu, "$section"), meter("RAM", ram, "$orange" if int(ram[:-1]) > 55 else "$section")],
            width="fill_container", padding=[10, 0], gap=14, alignItems="center", stroke="$border", strokeWidth={"top": 1})
    mesh = panel("Nodes", [panel_head("Nodes and mesh", "Clients, temperature and load per node.", text("Open topology", 12, "$section", "500")),
                           col("Rows", [node_row("mainrouter", "gateway · portal", "14%", "61%", 9, "WAN 1G to ONT", "$green", 73),
                                        node_row("ap-2", "wired + mesh", "6%", "38%", 8, "lan1 1G, mesh -75", "$yellow", 62),
                                        node_row("ap-4", "mesh only · 5 GHz", "4%", "34%", 6, "mesh -49, 1921 Mb", "$green", 62)], width="fill_container"),
                           row("Steer", [icon("shuffle", 13, "$section"), text("In the last 24 hours usteer moved 14 clients to 5 GHz and 3 to ap-4.", 12, "$text-2", textGrowth="fixed-width", width="fill_container")],
                               gap=8, padding=[8, 10], cornerRadius=6, fill="$section-soft", width="fill_container")], gap=10)

    # QoS / FUP
    tiers = [("< 50 GB", "uncapped", 17, "$green"), ("50-100 GB", "125 Mbit", 1, "$yellow"), ("100-200 GB", "62.5 Mbit", 0, "$orange"), ("200 GB +", "50 Mbit", 0, "$red")]
    qos = panel("QoS", [panel_head("Fair use", "Monthly tiers. Counting only, no limits enforced.", chip("Counting", "$green")),
                        col("Tiers", [frame(f"Tier {t}", [row("l", [rect("sw", 3, 22, c, 1), two(t, s, b_mono=True)], gap=10),
                                                           mono(str(n), 13, weight="600")], width="fill_container", justifyContent="space_between",
                                            alignItems="center") for t, s, n, c in tiers], gap=10, width="fill_container"),
                        row("Note", [icon("triangle-alert", 13, "$orange"), text("Shaper is off, so latency rises when the line is busy.", 12, "$text-2", textGrowth="fixed-width", width="fill_container")],
                            gap=8, width="fill_container", padding=[8, 10], cornerRadius=6, fill="$orange-soft")], width=330, gap=12)

    # DNS
    dns_vals = series(48, 850, 300, seed=4, smooth=0.7)
    dns = panel("DNS", [panel_head("DNS filter", "AdGuard Home with 4 block lists"),
                        row("Stats", [col("a", [label("Queries"), mono("41,203", 18, weight="600")], gap=4),
                                      col("b", [label("Blocked"), mono("18.4%", 18, "$red", "600")], gap=4),
                                      col("c", [label("Latency"), mono("14 ms", 18, weight="600")], gap=4)], gap=24),
                        sparkline("DNS Spark", 280, 44, dns_vals, "#FF5C5C"),
                        col("Top blocked", [row(f"D {d}", [mono(d, 11, "$text-2"), mono(n, 11)], width="fill_container", justifyContent="space_between")
                                            for d, n in [("app-measurement.com", "1,204"), ("doubleclick.net", "866"), ("tracking.miui.com", "612")]],
                            gap=6, width="fill_container")], width=312, gap=12)

    return screen("Dashboard", x, "Dashboard",
                  page_head("Dashboard", "All 3 nodes healthy. 23 devices online, 184 Mbps down right now.", range_controls()),
                  [kpis, row("Row 2", [bandwidth, talkers], gap=GAP, width="fill_container", alignItems="start"),
                   row("Row 3", [mesh, qos, dns], gap=GAP, width="fill_container", alignItems="start")])

# ------------------------------------------------------------------ WiFi
def wifi(x):
    kpis = kpi_band([
        kpi("Connected clients", "19", "latest per MAC", "wifi", SECTION["WiFi"]),
        kpi("Peak clients", "", "", "trending-up", "#4E8BFF", extra=row("Peaks", [col(f"p{l}", [mono(v, 18, weight="600"), label(l)], gap=3, alignItems="center")
                                                                       for v, l in [("21", "today"), ("24", "7d"), ("27", "all")]],
                                                             gap=28, width="fill_container", justifyContent="center")),
        kpi("SSIDs", "2", "Example-Primary and Example-Legacy", "radio", SECTION["Settings"]),
        kpi("Access points", "3", "1 wired, 2 on mesh", "router", SECTION["Topology"]),
        kpi("Signal mix", "", "", "activity", "#3DD68C",
            extra=col("Mix", [
                row("Tiers", [col(f"Tier {l}", [mono(str(v), 20, c, "600"), label(l)], gap=2)
                              for v, l, c in [(9, "strong", "$green"), (7, "fair", "$yellow"), (3, "weak", "$red")]],
                    width="fill_container", justifyContent="space_between", alignItems="start"),
                row("Mix Bar", [rect("strong", 90, 4, "$green", 2), rect("fair", 70, 4, "$yellow", 2), rect("weak", 28, 4, "$red", 2)],
                    gap=2, width="fill_container")], gap=8, width="fill_container"))])

    pw = C - 32 - 48
    a = series(120, 7, 3, seed=5, spike=0.0); b = series(120, 6, 2.5, seed=6, spike=0.0); c = series(120, 5, 2, seed=7, spike=0.0)
    dist = panel("Client Distribution", [
        panel_head("Client distribution", "Concurrent clients by access point. Drag across the chart to zoom.", seg(["By band", "By AP"], "By AP")),
        area_chart("Dist Chart", pw, 200, [(a, NODE["mainrouter"]), (b, NODE["ap-2"]), (c, NODE["ap-4"])], 30, lambda v: f"{v:.0f}", HOURS, stacked=True),
        legend([(n, c) for n, c in NODE.items()])])

    tp = panel("Throughput per AP", [
        panel_head("Throughput per access point", "Radio interface counters · 5m buckets", row("c", [seg(["Lines", "Stacked"], "Stacked"), seg(["Down", "Up", "Both"], "Both")], gap=8)),
        area_chart("TP Chart", pw, 190, [(series(120, 30, 25, seed=8, spike=0.01, smooth=0.75), NODE["mainrouter"]), (series(120, 18, 16, seed=10, spike=0.01, smooth=0.75), NODE["ap-2"]), (series(120, 22, 18, seed=12, spike=0.01, smooth=0.75), NODE["ap-4"])],
                   120, mbps, HOURS, mirror_max=30, mirror=[(series(120, 4, 4, seed=13, spike=0.01, smooth=0.75), NODE["mainrouter"]), (series(120, 3, 3, seed=14, spike=0.01, smooth=0.75), NODE["ap-2"]), (series(120, 3, 3, seed=15, spike=0.01, smooth=0.75), NODE["ap-4"])], stacked=True),
        legend([("mainrouter 41.2 GB ↓ 5.1 GB ↑", NODE["mainrouter"]), ("ap-2 22.8 GB ↓ 6.4 GB ↑", NODE["ap-2"]), ("ap-4 18.1 GB ↓ 1.9 GB ↑", NODE["ap-4"])])])

    radios = [("mainrouter", "2.4 GHz", "ch 6 · HE20", "20 dBm", "-92", 3, 0.18), ("mainrouter", "5 GHz", "ch 36 · HE80", "23 dBm", "-95", 6, 0.31),
              ("ap-2", "2.4 GHz", "ch 11 · HE20", "20 dBm", "-90", 4, 0.22), ("ap-2", "5 GHz", "ch 36 · HE80", "23 dBm", "-94", 4, 0.27),
              ("ap-4", "2.4 GHz", "disabled", "off", "off", 0, 0.0), ("ap-4", "5 GHz", "ch 36 · HE80", "23 dBm", "-96", 6, 0.24)]
    radio_tbl = panel("Radios", [
        frame("hp", [panel_head("Radios", "Per-radio RF state from iwinfo.")], width="fill_container", padding=[0, 16]),
        table("Radio Table", [("Node", "fill_container", "left"), ("Band", 80, "left"), ("Channel", 110, "left"), ("Noise", 70, "right"),
                              ("Clients", 64, "right"), ("Airtime", 110, "right")],
              [[node_tag(n), text(b, 12, "$text-2"), mono(ch, 11, "$text-3" if ch == "disabled" else "$text-2"), mono(nz, 11, "$text-2"),
                mono(str(cl), 12), row("air", [share_bar(air, 50, "$orange" if air > 0.3 else "$section"), mono(f"{air*100:.0f}%", 11)], gap=6)]
               for n, b, ch, tx, nz, cl, air in radios], row_h=40)], pad=[16, 0, 4, 0])

    mesh = panel("Mesh", [panel_head("Mesh backhaul", "phy1-mesh0 · station dump"),
                          *[col(f"Link {p}", [row("h", [row("pair", [node_tag("mainrouter"), text("to", 11, "$text-3"), node_tag(p.split()[-1])], gap=6), chip(q, qc)], width="fill_container", justifyContent="space_between"),
                                              row("m", [mono(f"{s} dBm", 11, "$text-2"), mono(f"{r} Mbps", 11, "$text-2"), mono(f"exp {e}", 11, "$text-3")], gap=14),
                                              sparkline(f"Spark {p}", 300, 28, series(40, sv, 3, seed=len(p), spike=0), "#3DD68C" if qc == "$green" else "#F2C14C")],
                                gap=8, width="fill_container", padding=[10, 0], stroke="$border", strokeWidth={"top": 1})
                            for p, q, qc, s, r, e, sv in [("mainrouter ↔ ap-4", "Strong", "$green", -49, 1921, "98%", 20),
                                                          ("mainrouter ↔ ap-2", "Fair", "$yellow", -75, 432, "71%", 12)]],
                          row("Steer", [icon("shuffle", 13, "$section"), text("usteer: roam at -70 dBm, kick below -82", 12, "$text-2")],
                              gap=8, padding=[8, 10], cornerRadius=6, fill="$section-soft", width="fill_container")], width=340)

    return screen("WiFi", x, "WiFi",
                  page_head("WiFi", "19 clients on 2 networks. ap-2's mesh link is the weak spot at -75 dBm.", range_controls(False)),
                  [kpis, dist, tp, row("Row", [radio_tbl, mesh], gap=GAP, width="fill_container", alignItems="start")])

# ------------------------------------------------------------------ Devices
def devices(x):
    filters = row("Filters", [
        row("Filter", [icon("search", 14, "$text-3"), text("Filter by name, IP or MAC", 13, "$text-3")], width=280, padding=[7, 10], gap=8,
            cornerRadius=7, stroke="$border", strokeWidth=1),
        seg(["All", "WiFi", "Wired", "Legacy IoT"], "All"), button("Active now"), button("Whitelisted", "shield-check")], gap=8)
    pw = C - 32 - 48
    tt = panel("Top Talkers Chart", [
        panel_head("Top talkers over time", "Busiest 5 devices as their own series; everyone else stacked as Others · 15m buckets.",
                   row("c", [seg(["Lines", "Stacked"], "Stacked"), seg(["Down", "Up", "Both"], "Both"), seg(["Top 5", "Top 10"], "Top 5")], gap=8)),
        area_chart("TT Chart", pw, 190, [(series(96, 14, 10, seed=20, spike=0.01, smooth=0.75), DEVICE[0]), (series(96, 8, 7, seed=21, spike=0.01, smooth=0.75), DEVICE[1]), (series(96, 5, 5, seed=22, spike=0.01, smooth=0.75), DEVICE[2]),
                                          (series(96, 3, 3, seed=23, spike=0.01, smooth=0.75), DEVICE[3]), (series(96, 6, 3, seed=24, spike=0), DEVICE[4])],
                   60, mbps, HOURS, mirror_max=10, mirror=[(series(96, 2, 2, seed=25, spike=0.01, smooth=0.75), DEVICE[0]), (series(96, 1.5, 1.5, seed=26, spike=0.01, smooth=0.75), DEVICE[1])], stacked=True),
        legend([("Client 01 64.2 GB", DEVICE[0]), ("Client 02 51.8 GB", DEVICE[1]), ("Client 03 22.4 GB", DEVICE[2]), ("Client 04 18.7 GB", DEVICE[3]),
                ("Others (19) 42.9 GB", DEVICE[4])])])

    cols = [("Device", "fill_container", "left"), ("Connection", 250, "left"), ("Down now", 100, "right"), ("Up now", 100, "right"),
            ("Month", 90, "right"), ("QoS", 130, "left"), ("Share", 120, "right")]
    rows_ = [[two(d[0], d[1]), conn_cell(d), mono(f"{d[5]} Mbps", 12), mono(f"{d[6]} Mbps", 12), mono(f"{d[7]} GB", 12, weight="600"),
              tier_chip(d[9]), row("s", [share_bar(d[8] / 0.3, 60, DEVICE[min(i, 4)]), mono(f"{d[8]*100:.1f}%", 11, "$text-2")], gap=8)] for i, d in enumerate(DEVICES)]
    all_tbl = panel("All Devices", [
        frame("hp", [panel_head("All devices", "Click a row for the device card.", mono("23 online · 11 offline", 11, "$text-3"))], width="fill_container", padding=[0, 16]),
        table("Devices Table", cols, rows_)], pad=[16, 0, 0, 0])

    return screen("Devices", x, "Devices",
                  page_head("Devices", "23 devices moved data this week. Client 01 used the most.", range_controls(rng="Last 7 days")),
                  [filters, tt, all_tbl])

# ------------------------------------------------------------------ Usage
def usage(x):
    kpis = kpi_band([
        kpi("Downloaded", "371.2 GB", "24 days in window", "arrow-down", DOWN),
        kpi("Uploaded", "41.5 GB", "WAN scope", "arrow-up", UP),
        kpi("Total", "412.7 GB", "15.9 Mbps average", "sigma", SECTION["Usage"]),
        kpi("Active devices", "34", "distinct in window", "monitor-smartphone", SECTION["Devices"]),
        kpi("Near fair-use limit", "1", "Client 02 at 51.8 of 50 GB", "gauge", "#FF8A4C", "$orange"),
        kpi("Speed test", "269 Mbps", "Median of 96 runs, 66 ms idle", "zap", SECTION["Traffic"])])

    pw = C - 32 - 48
    days = []
    for i in range(24):
        d = random.uniform(8, 26) if i not in (5, 12, 19) else random.uniform(28, 38)
        days.append([(d * 0.55, DEVICE[0]), (d * 0.3, DEVICE[1]), (d * 0.15, DEVICE[4])])
        days.append([(d * 0.11, UP)])
    per_day = panel("Usage Per Day", [
        panel_head("Usage per day", "Two columns per day: download left (stacked by device group), upload right.",
                   seg(["Down / Up", "Devices"], "Down / Up")),
        bar_chart("Day Chart", pw, 200, days, 40, gbf, ["Sep 1", "Sep 5", "Sep 9", "Sep 13", "Sep 17", "Sep 21", "Today"]),
        legend([("Laptops", DEVICE[0]), ("Media and TV", DEVICE[1]), ("Other", DEVICE[4]), ("Upload", UP)])])

    daily = panel("Daily", [
        frame("hp", [panel_head("Daily", "Newest first. From /etc/usage-history rollups.")], width="fill_container", padding=[0, 16]),
        table("Daily Table", [("Day", "fill_container", "left"), ("Down", 80, "right"), ("Up", 70, "right"), ("Total", 80, "right"),
                              ("Avg rate", 90, "right"), ("Devices", 70, "right")],
              [[row("d", [mono(d, 12), chip("today", "$section") if i == 0 else None], gap=8), mono(dn, 12), mono(up, 12), mono(t, 12, weight="600"),
                mono(r, 12, "$text-2"), mono(n, 12, "$text-2")]
               for i, (d, dn, up, t, r, n) in enumerate([("2026-09-24 Thu", "14.2 GB", "1.6 GB", "15.8 GB", "12.1 Mbps", "21"),
                                                         ("2026-09-23 Wed", "22.9 GB", "2.4 GB", "25.3 GB", "19.4 Mbps", "24"),
                                                         ("2026-09-22 Tue", "18.4 GB", "1.9 GB", "20.3 GB", "15.6 Mbps", "23"),
                                                         ("2026-09-21 Mon", "31.0 GB", "3.1 GB", "34.1 GB", "26.2 Mbps", "25"),
                                                         ("2026-09-20 Sun", "16.7 GB", "1.8 GB", "18.5 GB", "14.2 Mbps", "22")])], row_h=40)],
        pad=[16, 0, 4, 0])

    fup = panel("FUP", [
        frame("hp", [panel_head("Monthly fair use", "Per device · resets on the 1st")], width="fill_container", padding=[0, 16]),
        table("FUP Table", [("Device", "fill_container", "left"), ("Used", 120, "left"), ("Tier", 110, "left")],
              [[mono(d[0], 12), col("u", [mono(f"{d[7]} / 50 GB", 11), share_bar(min(1, d[7] / 50), 90, "$orange" if d[7] > 50 else "$section")], gap=4),
                tier_chip(d[9])] for d in DEVICES[:6]], row_h=44)], width=440, pad=[16, 0, 4, 0])

    return screen("Usage", x, "Usage",
                  page_head("Usage", "412.7 GB this month. Client 02 is past its 50 GB fair-use tier.",
                            row("c", [seg(["Daily", "Weekly", "Monthly"], "Daily"), seg(["7 days", "30 days", "90 days"], "30 days"), seg(["All", "WAN", "LAN"], "All")], gap=8)),
                  [kpis, per_day, row("Row", [daily, fup], gap=GAP, width="fill_container", alignItems="start")])

# ------------------------------------------------------------------ Topology
def topology(x):
    cw, ch = C - 340 - GAP, 540
    cx = cw // 2
    def node(name, sub, ic, nx, ny, sel=False):
        hue = NODE.get(name, "#8391A7")
        return row(f"Node {name}", [frame("Ico", [icon(ic, 18, "#FFFFFF" if sel else hue)], width=36, height=36, cornerRadius=8,
                                          fill=hue if sel else hue + "26", justifyContent="center", alignItems="center"),
                                    col("T", [mono(name, 13, weight="600"), text(sub, 11, "$text-3")], gap=3)],
                   x=nx, y=ny, width=210, padding=11, gap=11, fill="$raised", cornerRadius=10,
                   stroke=hue if sel else hue + "55", strokeWidth=1.5 if sel else 1)
    def tag(s, tx, ty, color="$text-2"):
        return row(f"Tag {s}", [mono(s, 10, color)], x=tx, y=ty, padding=[3, 7], cornerRadius=5, fill="$bg", stroke="$border", strokeWidth=1)
    def cluster(tx, ty, n):
        return row("Clients", [frame(f"c{i}", [icon(ic, 11, "$text-2")], width=22, height=22, cornerRadius=11, fill="$raised",
                                     stroke="$border", strokeWidth=1, justifyContent="center", alignItems="center")
                               for i, ic in enumerate(["smartphone", "laptop", "tv", "cctv"])] + [mono(f"+{n-4}", 10, "$text-2")], x=tx, y=ty, gap=4)
    kids = [
        rect("w ont", 2, 72, "$border-strong", x=cx - 1, y=90),
        rect("w trunk", 2, 72, "#3DD68C", x=cx - 1, y=220),
        rect("w branch", 420, 2, "#3DD68C", x=cx - 210, y=292),
        rect("w ap2", 2, 76, "#F2C14C", x=cx - 211, y=292),
        rect("w ap4", 2, 76, "#3DD68C", x=cx + 209, y=292),
        rect("w eth v", 2, 150, "$border-strong", x=cx - 251, y=218),
        rect("w eth h", 146, 2, "$border-strong", x=cx - 251, y=218),
        node("Jio ONT", "192.0.2.1, upstream", "globe", cx - 105, 32),
        node("mainrouter", "192.0.2.3 · gateway", "router", cx - 105, 162, True),
        node("ap-2", "192.0.2.9 · wired + mesh", "radio-tower", cx - 316, 368),
        node("ap-4", "192.0.2.15 · mesh only", "radio-tower", cx + 104, 368),
        tag("WAN · DHCP .59", cx + 10, 116),
        tag("-75 dBm · 432 Mbps", cx - 196, 262, "$yellow"),
        tag("-49 dBm · 1921 Mbps", cx + 48, 262, "$green"),
        tag("lan1 · 1 Gbps", cx - 300, 300),
        cluster(cx - 300, 438, 8), cluster(cx + 120, 438, 6), cluster(cx + 110, 176, 9),
        row("Map Legend", [row("l1", [rect("s", 14, 2, "#3DD68C"), text("mesh (strong)", 11, "$text-2")], gap=6),
                           row("l2", [rect("s", 14, 2, "#F2C14C"), text("mesh (fair)", 11, "$text-2")], gap=6),
                           row("l3", [rect("s", 14, 2, "$border-strong"), text("ethernet", 11, "$text-2")], gap=6)],
            x=16, y=ch - 46, gap=16, padding=[7, 10], cornerRadius=7, fill="$panel", stroke="$border", strokeWidth=1),
    ]
    canvas = frame("Map", kids, layout="none", width=cw, height=ch, fill="$panel", cornerRadius=10, stroke="$border", strokeWidth=1, clip=True)

    def kv(k, v): return row(f"KV {k}", [text(k, 12, "$text-3"), mono(v, 12)], width="fill_container", justifyContent="space_between")
    def meter(lbl_, pct, v, c="$section"):
        return col(f"M {lbl_}", [row("h", [label(lbl_), mono(v, 11, "$text-2")], width="fill_container", justifyContent="space_between"),
                                 share_bar(pct, 306, c)], gap=6, width="fill_container")
    detail = panel("Device Card", [
        row("Head", [frame("Ico", [icon("router", 20, "#FFFFFF")], width=40, height=40, cornerRadius=9, fill=NODE["mainrouter"],
                           justifyContent="center", alignItems="center"),
                     col("T", [mono("mainrouter", 15, weight="600"), text("JioRouter AX6000 · JIDU6401", 11, "$text-3")], gap=3)], gap=12),
        row("Chips", [chip("online 4d 2h", "$green"), chip("gateway", "$section"), chip("mesh portal", "$section")], gap=6),
        col("KV", [kv("LAN", "192.0.2.3"), kv("WAN", "192.0.2.2"), kv("Firmware", "25.12.5 r33051"), kv("Clients", "9 (3 × 2.4G, 6 × 5G)")],
            gap=9, width="fill_container"),
        col("Meters", [meter("cpu", 0.14, "14% · load 0.31"), meter("memory", 0.61, "302 / 495 MB", "$orange"), meter("overlay", 0.38, "38%")],
            gap=12, width="fill_container"),
        col("Temps", [row("th", [label("Temperatures"), text("hot at 120°", 10, "$text-3")], width="fill_container", justifyContent="space_between")] + [
            row(f"T {n}", [text(n, 12, "$text-2"), frame("sp", [], width="fill_container", height=1), share_bar(t / 120, 120, "$orange" if t >= 70 else "$green"),
                          mono(f"{t}°C", 12, "$orange" if t >= 70 else "$text", textGrowth="fixed-width", width=44, textAlign="right")], gap=10, width="fill_container")
            for n, t in [("CPU", 52), ("WiFi 2.4 GHz", 58), ("WiFi 5 GHz", 73)]],
            gap=12, width="fill_container"),
        col("Procs", [label("Top processes")] + [row(f"P {p}", [mono(p, 11, "$text-2"), mono(m, 11)], width="fill_container", justifyContent="space_between")
                                                 for p, m in [("AdGuardHome", "61 MB"), ("openwrtfi", "3.1 MB"), ("hostapd", "2.4 MB"), ("usteerd", "1.1 MB")]],
            gap=7, width="fill_container"),
        row("Actions", [button("Restart", "rotate-cw"), button("LuCI", "external-link"), button("Shell", "terminal", True)], gap=8)],
        width=340, gap=16)
    return screen("Topology", x, "Topology",
                  page_head("Topology", "Select a node to see its card. Line colour shows mesh link quality.", range_controls(False, "Live")),
                  [row("Body", [canvas, detail], gap=GAP, width="fill_container", alignItems="start")])

# ------------------------------------------------------------------ WiFi Networks (Settings)
def field(label_, value, ic=None, mono_val=False, note=None):
    input_row = row("Input", [(mono if mono_val else text)(value, 13, "$text"),
                              frame("sp", [], width="fill_container", height=1),
                              icon(ic, 14, "$text-3") if ic else None],
                    width="fill_container", padding=[8, 10], gap=8, cornerRadius=7,
                    stroke="$border", strokeWidth=1, alignItems="center")
    kids = [label(label_), input_row]
    if note: kids.append(text(note, 11, "$text-3"))
    return col(f"Field {label_}", kids, gap=6, width="fill_container")

def toggle_row(label_, on, note=None):
    track = frame("Track", [frame("Knob", [], width=14, height=14, fill="#FFFFFF", cornerRadius=7)],
                  width=32, height=18, padding=2, cornerRadius=9, alignItems="center",
                  justifyContent="end" if on else "start", fill="$section" if on else "$track")
    kids = [text(label_, 12, "$text", "500")]
    if note: kids.append(text(note, 11, "$text-3"))
    return row(f"Toggle {label_}", [track, col("L", kids, gap=2)], gap=10, alignItems="center", width="fill_container")

def mini_toggle(on, disabled=False):
    p = dict(width=26, height=15, padding=2, cornerRadius=8, alignItems="center",
             justifyContent="end" if (on and not disabled) else "start")
    if disabled:
        p.update(fill="$bg", stroke="$border", strokeWidth=1)
        knob = "$text-3"
    else:
        p.update(fill="$section" if on else "$track")
        knob = "#FFFFFF"
    return frame("Toggle", [frame("Knob", [], width=11, height=11, fill=knob, cornerRadius=6)], **p)

def matrix_row(node, s24, s5, note=None):
    r = row(f"MRow {node}", [frame("NodeCell", [node_tag(node)], width="fill_container"),
                             frame(f"C24 {node}", [mini_toggle(*s24)], width=64, justifyContent="center", alignItems="center"),
                             frame(f"C5 {node}", [mini_toggle(*s5)], width=64, justifyContent="center", alignItems="center")],
            width="fill_container", alignItems="center", gap=8)
    if not note: return r
    return col(f"MRowWrap {node}", [r, text(note, 11, "$orange", textGrowth="fixed-width", width="fill_container")], gap=4, width="fill_container")

def bcast_chip(short, state):
    hue = NODE["mainrouter" if short == "main" else short]
    label_ = {"full": short, "5g": f"{short} 5G", "off": f"{short} off"}[state]
    if state == "off":
        return row(f"B {short}", [mono(label_, 10, "$text-3")], padding=[2, 6], cornerRadius=5, stroke="$border", strokeWidth=1)
    return row(f"B {short}", [rect("sw", 3, 10, hue, 1), mono(label_, 10, "$text")], gap=5, padding=[2, 6], cornerRadius=5,
               fill=hue + "1F", stroke=hue + "55", strokeWidth=1)

def net_cell(name, sub, locked=False):
    title = row("Title", [icon("lock", 12, "$text-3") if locked else None,
                          text(name, 13, "$text", "600", textGrowth="fixed-width", width="fill_container")],
               gap=6, width="fill_container", alignItems="center")
    return col("Net", [title, text(sub, 11, "$text-3", textGrowth="fixed-width", width="fill_container")], gap=3, width="fill_container")

def sec_bands_cell(sec_label, sec_color, bands):
    return col("SecBands", [chip(sec_label, sec_color), text(bands, 11, "$text-3")], gap=4)

def cliroam_cell(clients, roam):
    return col("CliRoam", [mono(clients, 12, "$text", "600"), text(roam, 11, "$text-3")], gap=4)

def diff_line(node, kv):
    return row(f"Diff {node}", [mono(node, 11, "$text-3", textGrowth="fixed-width", width=78), mono(kv, 11, "$green")],
               gap=8, width="fill_container")

def rollout_row(node, status, color, ic):
    return row(f"Roll {node}", [icon(ic, 14, color), mono(node, 12, "$text-2", textGrowth="fixed-width", width=90),
                                frame("sp", [], width="fill_container", height=1), chip(status, color)],
              gap=8, width="fill_container", alignItems="center")

def wifi_networks(x):
    cols = [("Network", "fill_container", "left"), ("Security & band", 170, "left"),
            ("Broadcast on", 210, "left"), ("Clients / roaming", 150, "left")]
    rows_ = [
        [net_cell("Example-Primary", "Primary · WPA3-SAE mixed"), sec_bands_cell("WPA2/WPA3", "$section", "2.4 + 5 GHz"),
         row("Bc", [bcast_chip("main", "full"), bcast_chip("ap-2", "full"), bcast_chip("ap-4", "5g")], gap=4),
         cliroam_cell("16 clients", "r/k/v · usteer")],
        [net_cell("Example-Legacy", "IoT · 2.4 GHz only, not steered"), sec_bands_cell("WPA2-PSK", "$text-3", "2.4 GHz"),
         row("Bc", [bcast_chip("main", "full"), bcast_chip("ap-2", "full"), bcast_chip("ap-4", "off")], gap=4),
         cliroam_cell("3 clients", "not steered")],
        [net_cell("Example-Backhaul", "802.11s mesh backhaul · system", locked=True), sec_bands_cell("SAE (mesh)", "$text-3", "5 GHz only"),
         row("Bc", [bcast_chip("main", "5g"), bcast_chip("ap-2", "5g"), bcast_chip("ap-4", "5g")], gap=4),
         cliroam_cell("2 links", "system-managed")],
    ]
    networks_tbl = table("Networks Table", cols, rows_, row_h=72)
    networks_tbl["children"][1]["fill"] = "$section-soft"  # select Example-Primary row

    left_panel = panel("Networks", [
        frame("hp", [panel_head("Networks", "3 SSIDs across 3 nodes · uci show wireless")], width="fill_container", padding=[0, 16]),
        networks_tbl], pad=[16, 0, 4, 0])

    matrix_section = col("Matrix Section", [
        label("Broadcast matrix"),
        row("Matrix Head", [frame("h1", [label("Node")], width="fill_container"),
                            frame("h2", [label("2.4 GHz")], width=64, justifyContent="center"),
                            frame("h3", [label("5 GHz")], width=64, justifyContent="center")], width="fill_container", gap=8),
        matrix_row("mainrouter", (True, False), (True, False)),
        matrix_row("ap-2", (True, False), (True, False)),
        matrix_row("ap-4", (False, True), (True, False), note="radio0 disabled on ap-4")],
        gap=8, width="fill_container")

    advanced_section = col("Advanced Section", [
        label("Advanced · roaming"),
        toggle_row("802.11r FT-over-DS", True, "Mobility domain 2c3c · seamless roam"),
        toggle_row("802.11k/v neighbor reports", True),
        toggle_row("802.11w (PMF)", True, "Required"),
        toggle_row("usteer band steering", True, "Example-Primary only · roam below -70 dBm")],
        gap=10, width="fill_container")

    diff_section = col("Diff Section", [
        label("Pending changes"),
        frame("Diff Box", [diff_line("mainrouter", "wireless.default_radio1.key='••••••••••••'"),
                           diff_line("ap-2", "wireless.default_radio1.key='••••••••••••'"),
                           diff_line("ap-4", "wireless.default_radio1.key='••••••••••••'")],
              layout="vertical", gap=6, padding=10, cornerRadius=6, fill="$bg", stroke="$border", strokeWidth=1, width="fill_container")],
        gap=8, width="fill_container")

    rollout_section = col("Rollout Section", [
        label("Apply plan"),
        rollout_row("ap-4", "applying · rollback 28s", "$yellow", "loader-circle"),
        rollout_row("ap-2", "queued", "$text-3", "circle"),
        rollout_row("mainrouter", "queued", "$text-3", "circle"),
        row("Drawer Footer", [button("Cancel"), button("Apply to 3 nodes", "upload", True)],
            gap=8, width="fill_container", justifyContent="space_between")],
        gap=10, width="fill_container")

    right_drawer = panel("Edit Drawer", [
        panel_head("Edit network", "Example-Primary · applies to all 3 nodes", chip("selected", "$section")),
        field("Network name", "Example-Primary"),
        field("Password", "••••••••••••", ic="eye", mono_val=True, note="WPA2/WPA3-SAE mixed · 12 chars"),
        field("Security", "WPA2/WPA3 (SAE mixed)", ic="chevron-down"),
        row("Bands Row", [toggle_row("2.4 GHz", True), toggle_row("5 GHz", True)], gap=20, width="fill_container"),
        matrix_section, advanced_section, diff_section, rollout_section],
        width=420, gap=18, pad=16)

    return screen("WiFi Networks", x, "Settings",
                  page_head("WiFi networks", "One definition, pushed to every node. Changes auto-revert after 30 s unless confirmed.",
                            button("New network", "plus", True)),
                  [row("Networks Row", [left_panel, right_drawer], gap=GAP, width="fill_container", alignItems="start")])

# ------------------------------------------------------------------ tokens + doc
VARS = {
    "bg": "#0D1117", "sidebar": "#10141A", "panel": "#141920", "raised": "#1B212A", "grid": "#1E252E",
    "border": "#232B35", "border-strong": "#3A4452", "track": "#252D38",
    "text": "#E6EAF0", "text-2": "#9AA4B2", "text-3": "#6B7686",
    "accent": "#4E8BFF", "accent-soft": "#14253A",
    # traffic direction: blue/amber reads for colour-blind users where red/green doesn't
    "down": "#4EA8FF", "up": "#F2B14C",
    "green": "#3DD68C", "red": "#FF5C5C", "orange": "#FF8A4C", "orange-soft": "#2A1C14", "yellow": "#F2C14C",
    "ok": "#3DD68C",
}

def section_vars():
    """`$section` / `$section-soft` resolve per screen through the frame's `section` theme."""
    val = lambda suffix: [{"value": SECTION[k] + suffix, "theme": {"section": v}} for k, v in THEME.items()]
    return {"section": {"type": "color", "value": val("")}, "section-soft": {"type": "color", "value": val("1F")}}

def main():
    screens = [dashboard(0), wifi(W + 100), devices(2 * (W + 100)), usage(3 * (W + 100)), topology(4 * (W + 100)),
               wifi_networks(5 * (W + 100))]
    doc = {"version": "2.6", "themes": {"section": list(THEME.values())},
           "variables": {**{k: {"type": "color", "value": v} for k, v in VARS.items()}, **section_vars()}, "children": screens}
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "openwrtfi.pen")
    with open(out, "w") as f: json.dump(doc, f, indent=1)
    print("wrote", out, next(_ids) - 1, "nodes")

if __name__ == "__main__":
    main()
