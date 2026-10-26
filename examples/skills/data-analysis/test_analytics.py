"""Checks for analytics.py: run with scripts/skills-test.sh (in the code
sandbox image). Builds data with a known answer and checks it is found."""

import os
import tempfile

import numpy as np
import pandas as pd

import analytics


def signups(path):
    # Two weeks; ads collapses in the second, organic and referral hold.
    rows = []
    for day in pd.date_range("2026-10-01", periods=14, freq="D"):
        for channel, n in [("organic", 40), ("ads", 30 if day < pd.Timestamp("2026-10-08") else 5), ("referral", 10)]:
            for plan, share in [("free", 0.8), ("pro", 0.2)]:
                rows.append({"day": day.date(), "channel": channel, "plan": plan, "signups": round(n * share)})
    pd.DataFrame(rows).to_csv(path, index=False)


def test_drivers(tmp):
    csv = os.path.join(tmp, "signups.csv")
    signups(csv)
    out = analytics.drivers(csv, date="day", dims=["channel", "plan", "missing"], metric="signups", chart=os.path.join(tmp, "d.png"))
    top = out.iloc[0]
    assert (top["dimension"], top["value"]) == ("channel", "ads"), out
    # ads lost 25 a day for 7 days: the whole drop.
    assert top["change"] == -175, out
    assert abs(top["share_of_change"] - 100) < 1e-6, out
    assert os.path.exists(os.path.join(tmp, "d.png"))
    # Counting rows instead of adding a column.
    rows = analytics.drivers(csv, date="day", dims=["channel"], chart=os.path.join(tmp, "d2.png"))
    assert rows["change"].abs().sum() == 0, "one row per day and segment either way"


def test_forecast(tmp):
    days = pd.date_range("2026-08-01", periods=70, freq="D")
    weekly = np.array([10, 12, 14, 13, 11, 4, 3] * 10, dtype=float)
    values = 100 + np.arange(70) * 0.5 + weekly
    csv = os.path.join(tmp, "daily.csv")
    pd.DataFrame({"day": days, "orders": values}).to_csv(csv, index=False)
    out = analytics.forecast(csv, "day", "orders", periods=14, chart=os.path.join(tmp, "f.png"))
    assert len(out) == 14
    expected = 100 + np.arange(70, 84) * 0.5 + weekly[:14]
    err = np.abs(out["forecast"].to_numpy() - expected).mean()
    assert err < 3, (err, out)
    assert (out["low_95"] <= out["forecast"]).all() and (out["forecast"] <= out["high_95"]).all()


def test_anomalies(tmp):
    days = pd.date_range("2026-09-01", periods=40, freq="D")
    rng = np.random.default_rng(1)
    values = 200 + rng.normal(0, 3, 40)
    values[30] = 40
    csv = os.path.join(tmp, "a.csv")
    pd.DataFrame({"day": days, "visits": values}).to_csv(csv, index=False)
    out = analytics.anomalies(csv, "day", "visits", chart=os.path.join(tmp, "a.png"))
    worst = out.loc[out["z"].abs().idxmax()]
    assert worst["day"].strftime("%Y-%m-%d") == "2026-10-01" and worst["z"] < -10, out
    assert len(out) <= 2, out


def test_quality(tmp, capsys=None):
    csv = os.path.join(tmp, "q.csv")
    df = pd.DataFrame(
        {
            "day": ["2026-10-01", "2026-10-02", "2026-10-04", "2026-10-04", "2026-10-05"] * 4,
            "user_id": list(range(19)) + [3],
            "country": [None] * 20,
            "amount": [10, 12, -5] + [11, 9, 10, 12, 11] * 3 + [9, 10],
            "source": ["web"] * 20,
        }
    )
    df.to_csv(csv, index=False)
    import contextlib
    import io

    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        out = analytics.quality(csv, date="day", key="user_id")
    text = buf.getvalue()
    for want in ["`country` is empty", "`source` has one value only", "1 duplicate keys", "1 days have no rows (2026-10-03)", "negative values", "newest row is from 2026-10-05"]:
        assert want in text, (want, text)
    assert len(out) == 5


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as tmp:
        for name, fn in list(globals().items()):
            if name.startswith("test_"):
                fn(tmp)
                print(f"ok - {name}")
