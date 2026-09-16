CREATE TABLE installer_fetches (
    day TEXT PRIMARY KEY,
    fetches INTEGER NOT NULL CHECK (fetches > 0)
) WITHOUT ROWID;

CREATE TABLE metric_snapshots (
    measured_at TEXT NOT NULL,
    source TEXT NOT NULL,
    metric TEXT NOT NULL,
    value INTEGER NOT NULL CHECK (value >= 0),
    window_start TEXT NOT NULL,
    window_end TEXT NOT NULL,
    PRIMARY KEY (measured_at, source, metric)
) WITHOUT ROWID;

INSERT INTO metric_snapshots
    (measured_at, source, metric, value, window_start, window_end)
VALUES
    ('2026-09-16T14:27:50Z', 'cloudflare', 'retained_installer_endpoint_requests', 8,
     '2026-09-06T15:06:53Z', '2026-09-16T14:27:50Z');
