from datetime import datetime, timezone


def parse_iso(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    if parsed.year < 2000:
        return None
    return parsed


def age(value: str | datetime | None) -> str:
    parsed = value if isinstance(value, datetime) else parse_iso(value)
    if parsed is None:
        return "-"
    seconds = max(0, int((datetime.now(timezone.utc) - parsed).total_seconds()))
    for limit, unit in ((86400, "d"), (3600, "h"), (60, "m")):
        if seconds >= limit:
            return f"{seconds // limit}{unit}"
    return f"{seconds}s"
