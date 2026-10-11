// Time-series area chart on uPlot: multiple series, optional stacking, an optional mirrored lower
// band (its own scale, e.g. upload drawn below the axis), per-series colour, tooltip, drag-to-zoom.
import "uplot/dist/uPlot.min.css";
import uPlot from "uplot";
import { useEffect, useRef } from "preact/hooks";

export interface AreaSeries { label: string; color: string; values: number[] }

export interface AreaChartProps {
  timestamps: number[];             // unix seconds, one per data point
  series: AreaSeries[];             // drawn above the baseline
  mirror?: AreaSeries[];            // drawn below the baseline, on its own scale
  stacked?: boolean;                // stack `series` (bands fill between cumulative lines)
  mirrorFrac?: number;              // fraction of the plot height the mirrored band occupies (default 0.3)
  height?: number;
  valueFmt?: (v: number) => string;
  xFmt?: (ts: number) => string;
  zoomable?: boolean;               // drag-to-zoom on the x axis (default true)
}

export function AreaChart({
  timestamps, series, mirror, stacked = false, mirrorFrac = 0.3, height = 200,
  valueFmt = (v) => v.toFixed(0), xFmt, zoomable = true,
}: AreaChartProps) {
  const holder = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const fullX = useRef<[number, number]>([timestamps[0] ?? 0, timestamps[timestamps.length - 1] ?? 1]);

  useEffect(() => {
    if (!holder.current) return;
    fullX.current = [timestamps[0] ?? 0, timestamps[timestamps.length - 1] ?? 1];

    const cumulative = (list: AreaSeries[]) => {
      const base = new Array(timestamps.length).fill(0);
      return list.map((s) => {
        if (!stacked) return s.values;
        for (let i = 0; i < s.values.length; i++) base[i] += s.values[i] ?? 0;
        return [...base];
      });
    };
    const mainVals = cumulative(series);
    const mirrorVals = (mirror ?? []).map((s) => s.values.map((v) => -Math.abs(v)));

    const data: uPlot.AlignedData = [timestamps, ...mainVals, ...mirrorVals] as unknown as uPlot.AlignedData;

    const uSeries: uPlot.Series[] = [{}];
    const bands: uPlot.Band[] = [];
    series.forEach((s, i) => {
      uSeries.push({
        label: s.label, stroke: s.color, width: 1.4, fill: `${s.color}26`,
        scale: "y", points: { show: false },
      });
      if (stacked && i > 0) bands.push({ series: [i, i + 1], fill: `${s.color}26` });
    });
    (mirror ?? []).forEach((s) => {
      uSeries.push({ label: s.label, stroke: s.color, width: 1.4, fill: `${s.color}26`, scale: "y2", points: { show: false } });
    });

    const mainMax = Math.max(1, ...mainVals.flat());
    const mirrorMax = Math.max(1, ...(mirror ?? []).flatMap((s) => s.values));

    const opts: uPlot.Options = {
      width: holder.current.clientWidth || 600,
      height,
      padding: [8, 8, 0, 0],
      cursor: {
        drag: { x: !!zoomable, y: false },
        points: { size: 6 },
      },
      legend: { show: false },
      scales: {
        x: { time: true },
        y: { range: () => [0, mainMax * 1.15] },
        y2: {
          range: () => {
            const m = mirrorMax * 1.15;
            const top = m * (1 - mirrorFrac) / mirrorFrac;
            return [-m, top];
          },
        },
      },
      axes: [
        {
          stroke: "#6B7686", grid: { stroke: "#1E252E", width: 1 },
          ticks: { stroke: "#1E252E" },
          values: (_u, ticks) => ticks.map((t) => (xFmt ? xFmt(t) : new Date(t * 1000).toLocaleTimeString([], { hour: "2-digit" }))),
        },
        {
          scale: "y", stroke: "#6B7686", grid: { stroke: "#1E252E", width: 1 }, ticks: { show: false },
          values: (_u, ticks) => ticks.map((t) => valueFmt(t)), size: 42,
        },
        ...(mirror && mirror.length
          ? [{ scale: "y2", stroke: "#6B7686", ticks: { show: false }, grid: { show: false }, values: (_u: uPlot, ticks: number[]) => ticks.map((t) => valueFmt(Math.abs(t))), size: 42 } as uPlot.Axis]
          : []),
      ],
      series: uSeries,
      bands,
      hooks: {
        setSelect: [
          (u) => {
            if (!zoomable || !u.select.width) return;
            const min = u.posToVal(u.select.left, "x");
            const max = u.posToVal(u.select.left + u.select.width, "x");
            u.setScale("x", { min, max });
            u.setSelect({ left: 0, top: 0, width: 0, height: 0 }, false);
          },
        ],
      },
    };

    const u = new uPlot(opts, data, holder.current);
    plot.current = u;

    const dblclick = () => u.setScale("x", { min: fullX.current[0], max: fullX.current[1] });
    if (zoomable) u.over.addEventListener("dblclick", dblclick);

    const ro = new ResizeObserver(() => {
      if (holder.current) u.setSize({ width: holder.current.clientWidth, height });
    });
    ro.observe(holder.current);

    return () => {
      ro.disconnect();
      if (zoomable) u.over.removeEventListener("dblclick", dblclick);
      u.destroy();
      plot.current = null;
    };
    // uPlot draws statically (no built-in animation), so prefers-reduced-motion needs no special case here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [timestamps, series, mirror, stacked, mirrorFrac, height, zoomable]);

  return <div class="area-chart" ref={holder} />;
}
