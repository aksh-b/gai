"""Analytics over CSV files: drivers of a change, forecasts, anomalies and
data quality. Needs pandas, numpy and matplotlib (the code sandbox image
has them). Each function prints Markdown, saves a PNG and returns a
DataFrame."""

import itertools
import math

import numpy as np
import pandas as pd

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402


def _read(csv, date=None):
    df = pd.read_csv(csv)
    if date is not None:
        if date not in df.columns:
            raise ValueError(f"no column {date!r}; columns: {', '.join(df.columns)}")
        df[date] = pd.to_datetime(df[date], errors="coerce")
        df = df.dropna(subset=[date]).sort_values(date)
    return df


def _table(df, floatfmt=".1f"):
    try:
        return df.to_markdown(index=False, floatfmt=floatfmt)
    except ImportError:
        return df.to_string(index=False)


def _save(fig, name):
    fig.tight_layout()
    fig.savefig(name, dpi=120)
    plt.close(fig)
    print(f"\n(chart saved as {name})")


def _series(df, date, value):
    if value not in df.columns:
        raise ValueError(f"no column {value!r}; columns: {', '.join(df.columns)}")
    s = df.groupby(date)[value].sum().sort_index()
    return s.astype(float)


# ---- drivers ----


def drivers(csv, date, dims, metric=None, agg="sum", days=7, current=None, previous=None, top=5, chart="drivers.png"):
    """Which values of each dimension explain the change between two periods."""
    df = _read(csv, date)
    if df.empty:
        print("No rows.")
        return pd.DataFrame()
    if metric is None:
        df = df.assign(_rows=1)
        metric, agg = "_rows", "sum"
    if current is None:
        end = df[date].max().normalize()
        cur = (end - pd.Timedelta(days=days - 1), end)
    else:
        cur = tuple(pd.Timestamp(x) for x in current)
    if previous is None:
        span = cur[1] - cur[0]
        prev = (cur[0] - span - pd.Timedelta(days=1), cur[0] - pd.Timedelta(days=1))
    else:
        prev = tuple(pd.Timestamp(x) for x in previous)

    def window(r):
        return df[(df[date] >= r[0]) & (df[date] <= r[1] + pd.Timedelta(hours=23, minutes=59, seconds=59))]

    a, b = window(prev), window(cur)
    fn = {"sum": "sum", "count": "count", "mean": "mean"}[agg]
    before, after = getattr(a[metric], fn)(), getattr(b[metric], fn)()
    total = after - before
    pct = (total / before * 100) if before else float("nan")
    label = "rows" if metric == "_rows" else metric
    print(
        f"**{label}**: {after:,.0f} from {cur[0]:%b %d} to {cur[1]:%b %d}, against {before:,.0f} "
        f"from {prev[0]:%b %d} to {prev[1]:%b %d}: {total:+,.0f} ({pct:+.1f}%)."
    )
    rows = []
    for d in dims:
        if d not in df.columns:
            print(f"\n(no column {d!r}; skipped)")
            continue
        g = pd.DataFrame(
            {
                "before": getattr(a.groupby(d)[metric], fn)(),
                "after": getattr(b.groupby(d)[metric], fn)(),
            }
        ).fillna(0)
        g["change"] = g["after"] - g["before"]
        g["share_of_change"] = g["change"] / total * 100 if total else 0.0
        g["pct_change"] = np.where(g["before"] != 0, g["change"] / g["before"].replace(0, np.nan) * 100, np.nan)
        g = g.reindex(g["change"].abs().sort_values(ascending=False).index)
        for value, r in g.head(top).iterrows():
            rows.append({"dimension": d, "value": value, **r.to_dict()})
    out = pd.DataFrame(rows)
    if out.empty:
        print("\nNo dimensions to break it down by.")
        return out
    # The dimension whose top value explains the most is the likeliest cause.
    out["abs"] = out["change"].abs()
    best = out.sort_values("abs", ascending=False).iloc[0]
    print(
        f"\nBiggest driver: **{best['dimension']} = {best['value']}**, {best['change']:+,.0f} "
        f"({best['share_of_change']:.0f}% of the change; {best['before']:,.0f} → {best['after']:,.0f})."
    )
    out = out.drop(columns="abs")
    print()
    print(_table(out))
    fig, ax = plt.subplots(figsize=(8, max(3, 0.35 * len(out))))
    labels = [f"{r.dimension}={r.value}" for r in out.itertuples()]
    ax.barh(labels[::-1], out["change"][::-1], color=["#d6336c" if c < 0 else "#2f9e44" for c in out["change"][::-1]])
    ax.axvline(0, color="#888", linewidth=0.8)
    ax.set_title(f"Change in {label} by segment")
    _save(fig, chart)
    return out


# ---- forecast ----


def _holt_winters(y, season, alpha, beta, gamma, periods):
    n = len(y)
    if season and n >= 2 * season:
        level = y[:season].mean()
        trend = (y[season : 2 * season].mean() - y[:season].mean()) / season
        seas = list(y[:season] - level)
    else:
        season = 0
        level, trend, seas = y[0], (y[-1] - y[0]) / max(n - 1, 1), []
    fitted = []
    for t in range(n):
        s = seas[t % season] if season else 0.0
        fitted.append(level + trend + s)
        last = level
        level = alpha * (y[t] - s) + (1 - alpha) * (level + trend)
        trend = beta * (level - last) + (1 - beta) * trend
        if season:
            seas[t % season] = gamma * (y[t] - level) + (1 - gamma) * s
    fc = [level + (h + 1) * trend + (seas[(n + h) % season] if season else 0.0) for h in range(periods)]
    return np.array(fitted), np.array(fc)


def forecast(csv, date, value, periods=14, season=None, chart="forecast.png"):
    """Holt-Winters forecast with 80% and 95% ranges."""
    df = _read(csv, date)
    s = _series(df, date, value)
    if len(s) < 4:
        raise ValueError("a forecast needs at least 4 points")
    step = s.index.to_series().diff().median()
    if season is None:
        season = 7 if step <= pd.Timedelta(days=1) else (52 if step <= pd.Timedelta(days=7) else 12)
    y = s.to_numpy()
    best = None
    grid = [0.1, 0.3, 0.5, 0.8]
    for a, b, g in itertools.product(grid, [0.0, 0.05, 0.2], [0.05, 0.2, 0.5]):
        fitted, fc = _holt_winters(y, season, a, b, g, periods)
        sse = float(((y - fitted) ** 2)[1:].sum())
        if best is None or sse < best[0]:
            best = (sse, fitted, fc, (a, b, g))
    sse, fitted, fc, params = best
    sigma = math.sqrt(sse / max(len(y) - 1, 1))
    h = np.arange(1, periods + 1)
    spread = sigma * np.sqrt(h)
    idx = pd.date_range(s.index[-1] + step, periods=periods, freq=step)
    out = pd.DataFrame(
        {
            date: idx,
            "forecast": fc,
            "low_80": fc - 1.28 * spread,
            "high_80": fc + 1.28 * spread,
            "low_95": fc - 1.96 * spread,
            "high_95": fc + 1.96 * spread,
        }
    )
    used = season if len(y) >= 2 * season else 0
    print(
        f"**{value}** over the next {periods} points: {fc.sum():,.0f} in total, {fc[-1]:,.1f} at the end "
        f"(now {y[-1]:,.1f}). Holt-Winters{f' with a season of {used}' if used else ' (too little history for a season)'}; "
        f"typical error {sigma:,.1f}."
    )
    print()
    print(_table(out.assign(**{date: out[date].dt.strftime("%Y-%m-%d")})))
    fig, ax = plt.subplots(figsize=(9, 4))
    ax.plot(s.index, y, label="actual")
    ax.plot(idx, fc, label="forecast")
    ax.fill_between(idx, out["low_95"], out["high_95"], alpha=0.15, label="95%")
    ax.fill_between(idx, out["low_80"], out["high_80"], alpha=0.25, label="80%")
    ax.set_title(f"{value}: forecast")
    ax.legend()
    _save(fig, chart)
    return out


# ---- anomalies ----


def anomalies(csv, date, value, window=14, threshold=3.5, chart="anomalies.png"):
    """Points far from the rolling median (robust z-score)."""
    df = _read(csv, date)
    s = _series(df, date, value)
    med = s.rolling(window, min_periods=max(3, window // 2), center=False).median().shift(1)
    # How far points usually stray, over the whole series: a few points'
    # spread is too noisy to judge by.
    resid = s - med
    mad = (resid - resid.median()).abs().median()
    z = 0.6745 * resid / (mad if mad else np.nan)
    out = pd.DataFrame({date: s.index, value: s.to_numpy(), "expected": med.to_numpy(), "z": z.to_numpy()})
    flagged = out[out["z"].abs() >= threshold].copy()
    if flagged.empty:
        print(f"No unusual points in **{value}** (robust z ≥ {threshold}) over {len(s)} points.")
    else:
        recent = flagged[date].max()
        print(
            f"{len(flagged)} unusual points in **{value}**; the latest on {recent:%Y-%m-%d}"
            f"{' (that is the newest data)' if recent == s.index.max() else ''}."
        )
        print()
        print(_table(flagged.assign(**{date: flagged[date].dt.strftime("%Y-%m-%d")})))
    fig, ax = plt.subplots(figsize=(9, 4))
    ax.plot(s.index, s.to_numpy(), label=value)
    ax.plot(s.index, med.to_numpy(), linestyle="--", label="rolling median")
    ax.scatter(flagged[date], flagged[value], color="#d6336c", zorder=3, label="unusual")
    ax.set_title(f"{value}: unusual points")
    ax.legend()
    _save(fig, chart)
    return flagged


# ---- quality ----


def quality(csv, date=None, key=None):
    """Problems that make numbers wrong: gaps, duplicates, empty columns."""
    df = _read(csv, date)
    issues = []
    rows = []
    for c in df.columns:
        nulls = df[c].isna().mean() * 100
        distinct = df[c].nunique(dropna=True)
        rows.append({"column": c, "type": str(df[c].dtype), "missing_%": nulls, "distinct": distinct})
        if nulls == 100:
            issues.append(f"`{c}` is empty")
        elif nulls >= 20:
            issues.append(f"`{c}` is missing in {nulls:.0f}% of rows")
        if distinct == 1 and len(df) > 1:
            issues.append(f"`{c}` has one value only ({df[c].dropna().iloc[0]!r})")
        if pd.api.types.is_numeric_dtype(df[c]) and (df[c] < 0).any() and (df[c] >= 0).mean() >= 0.9:
            issues.append(f"`{c}` has {(df[c] < 0).sum()} negative values among positive ones")
    dups = df.duplicated(subset=[key] if key else None).sum()
    if dups:
        issues.append(f"{dups} duplicate {'keys' if key else 'rows'}")
    if date is not None and not df.empty:
        days = df[date].dt.normalize()
        full = pd.date_range(days.min(), days.max(), freq="D")
        missing = full.difference(days.unique())
        if len(missing):
            show = ", ".join(d.strftime("%Y-%m-%d") for d in missing[:10])
            issues.append(f"{len(missing)} days have no rows ({show}{'…' if len(missing) > 10 else ''})")
        per_day = days.value_counts().sort_index()
        typical = per_day.median()
        low = per_day[per_day < typical * 0.3]
        if len(low):
            issues.append(
                f"{len(low)} days have under 30% of the usual {typical:.0f} rows ("
                + ", ".join(d.strftime("%Y-%m-%d") for d in low.index[:5])
                + ")"
            )
        issues.append(f"the newest row is from {days.max():%Y-%m-%d}")
    print(f"**{len(df)} rows, {len(df.columns)} columns.**")
    if issues:
        print("\n" + "\n".join(f"- {i}" for i in issues))
    print()
    out = pd.DataFrame(rows)
    print(_table(out))
    return out
