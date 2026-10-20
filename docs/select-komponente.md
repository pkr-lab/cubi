# Select-Komponente

Alle Auswahlen in Cubi laufen über dieselbe Komponente. Sie nutzt keine zusätzliche Bibliothek, nur `rich` für Stile und Zeilenbreiten, und zeichnet inline im normalen Terminal-Buffer.

## Dateien

| Datei | Aufgabe |
| --- | --- |
| `src/cubi/term.py` | Tastaturlesen, Terminal-Modus, `Region` (Zeichenbereich), ANSI-Rendering |
| `src/cubi/choice.py` | `Viewport`, `ListPrompt`, `select`, `multiselect`, `table_select` |
| `src/cubi/ui.py` | Fassade für die Module, dazu `prompt`, `confirm`, `text_block`, Ausgabe-Helfer |
| `src/cubi/argocd_apps.py` | `choose_apps`: Mehrfachauswahl von ArgoCD-Apps mit frei wählbaren Spalten |

## Zeichnen ohne Vollbild

`term.Region` merkt sich, wie viele Zeilen die letzte Ausgabe hatte. Ein neuer Frame bewegt den Cursor mit `ESC[nA` an den Anfang dieses Bereichs, löscht ab dort mit `ESC[J` (nur unterhalb des Cursors) und schreibt die neuen Zeilen. Der Text oberhalb wird nie angefasst. Es gibt weder `ESC[?1049h` (Alternate Screen) noch `ESC[2J` (Clear).

Jede Zeile wird auf die Terminalbreite abgeschnitten und nie umgebrochen. Nur so stimmt die Zeilenzahl, die `Region` zurückbewegt.

`Region.finish(zeile)` ersetzt den ganzen Bereich durch eine Zusammenfassungszeile (`? Frage › Antwort`). `Region.erase()` entfernt ihn spurlos. Letzteres passiert, wenn ein Hotkey die Ansicht verlässt.

`term.interactive()` setzt das Terminal auf zeichenweises Lesen ohne Echo und ohne Signalerzeugung (Ctrl+C kommt als Taste `ctrl-c` an), blendet den Cursor aus und stellt beim Verlassen alles wieder her, auch bei Ausnahmen.

## Aufbau von `ListPrompt`

`ListPrompt` hält den Zustand und rendert, ohne selbst zu lesen oder zu schreiben. Dadurch ist es ohne Terminal testbar.

- `handle(taste)` verarbeitet eine Taste und liefert `None` (weiter) oder ein Ergebnis `(aktion, wert)`: `"enter"`, `"back"`, `"reload"` oder den Hotkey.
- `lines(breite)` liefert die Zeilen des aktuellen Frames: Frage, optionale Untertitel/Zusammenfassung, Kopfzeile, Fenster mit Zeilen, Fußzeile.
- `reload()` ruft den Loader auf und behält Cursor-Wert und Häkchen bei.
- `Viewport` verwaltet Cursor und Fensterversatz (Scrollen, Seitensprünge, Umbruch an den Rändern).

`choice.run_list(prompt)` ist die Schleife: Frame zeichnen, Taste lesen, `handle` aufrufen, bei Ergebnis `Region.finish` oder `erase`. Mit `interval` lädt sie bei Inaktivität automatisch neu.

## Verwendung in Modulen

Einfachauswahl:

```python
from cubi import ui
from cubi.ui import Item

wert = ui.select("App wählen", [Item("my-app", "my-app", "default")])
```

Rückgabe ist der `value` des Eintrags oder `None` bei Esc.

Mehrfachauswahl:

```python
namen = ui.multiselect(
    "Labels hinzufügen",
    [Item(name, name) for name in labels],
    checked=["bug"],
    min_count=1,
)
```

Rückgabe ist eine Liste der Werte in Listenreihenfolge oder `None` bei Abbruch.

Tabelle mit Spalten, Nachladen und Hotkeys (auch als Mehrfachauswahl mit `multi=True`):

```python
def loader() -> TableData:
    rows = [Row([Text(app["name"]), Text(app["sync"])], app["name"]) for app in fetch()]
    return TableData(rows, Text(f"{len(rows)} Apps", style="dim"))

key, value = ui.table_select(
    "ArgoCD",
    ["App", "Sync"],
    loader,
    hotkeys={"f": "Filter"},
    interval=10,
)
```

`key` ist `"enter"`, `"back"` oder ein Hotkey. Bei `multi=True` ist `value` die Liste der gewählten Werte. `TableData(..., checked=[...])` wählt beim ersten Laden Einträge vor. Hotkeys dürfen in Mehrfachauswahlen weder `a` noch Leerzeichen belegen.

`Row.label` bestimmt den Text in der Zusammenfassungszeile. Ohne Angabe gilt die erste Zelle.

Weitere Eingaben: `ui.prompt(frage, vorgabe)` (Textzeile, Esc gibt `None`), `ui.confirm(frage, default=False)` (y/N), `ui.text_block(frage)` (mehrzeilig, leere Zeile beendet).

Fehler: `CubiError` im Loader erscheint als rote Zeile unter der Frage. `ui.guard(funktion, ...)` fängt `CubiError` und gibt ein Fehlerfeld aus.

## Tests

```bash
python3 -m unittest discover -s tests
```

| Datei | Inhalt |
| --- | --- |
| `tests/test_choice.py` | Viewport, Einfach- und Mehrfachauswahl, Rendering, Loader |
| `tests/test_term.py` | Tastenerkennung, Zeilenzählung |
| `tests/test_inline_pty.py` | echtes Pseudo-Terminal: keine Alt-Screen-/Clear-Sequenzen, Zusammenfassungszeile, Abbruch, Terminal-Modus danach, Cursor wieder sichtbar, Ctrl+C im Einstiegspunkt |
| `tests/test_no_fullscreen.py` | durchsucht den Quelltext nach Alternate Screen, Clear, Pager, Editor, Browser und Kommentaren |
| `tests/test_autosync.py`, `tests/test_branch.py` | Patch-Aufbau, Wiederherstellung, Warnungen, Remote-Prüfung (gemockt) |
