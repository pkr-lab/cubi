# Bedienung

## Terminal-Verhalten

Cubi bleibt immer in der normalen Terminal-Ansicht:

- kein Alternate Screen, kein Vollbild, kein Löschen des Terminal-Inhalts
- keine externen Programme wie Pager, Editor oder Browser
- alles erscheint inline unter dem aufgerufenen Befehl; der Verlauf bleibt nach dem Beenden vollständig sichtbar

Eine Auswahl zeichnet nur ihren eigenen Bereich neu. Der Text darüber bleibt unverändert. Nach der Entscheidung wird der Bereich durch eine einzelne Zeile ersetzt:

```
$ cubi argocd
? ArgoCD · Kontext kubernetes › my-app
```

Ein Abbruch hinterlässt `✖ <Frage> · abgebrochen`.

Was früher einen Pager, Editor oder Browser öffnete, geht jetzt inline:

| Vorher | Jetzt |
| --- | --- |
| Lange Ausgaben im Pager (`less`) | werden vollständig ins Terminal geschrieben |
| Issue-Beschreibung und Kommentare im `$EDITOR` | Eingabe im Terminal, eine leere Zeile beendet den Text |
| „Im Browser öffnen“ (`gh ... --web`, `gh browse`) | „Link anzeigen“ gibt die URL aus |
| `gh auth login` öffnet den Browser | Der Browser wird unterdrückt, der Code erscheint im Terminal, die URL ist `https://github.com/login/device` |
| ArgoCD-Login mit SSO (öffnet den Browser) | entfällt, der Login mit Passwort bleibt |

`ssh` (Connect, Tailscale) und `kubectl exec` übernehmen das Terminal, weil genau das ihr Zweck ist. Sie laufen ebenfalls inline im normalen Buffer.

## Tasten

| Taste | Wirkung |
| --- | --- |
| ↑ / ↓ | Eintrag wechseln (am Rand springt der Cursor ans andere Ende) |
| Bild ↑ / Bild ↓ | eine Fensterhöhe springen |
| Pos1 / Ende | zum ersten / letzten Eintrag |
| Enter | bestätigen |
| Esc | abbrechen bzw. eine Ebene zurück |
| Ctrl+C | abbrechen, Cubi beendet sich mit Exit-Code 130 (kein Stacktrace) |
| `r` | Tabelle neu laden (nur Tabellen) |

Die Hotkeys einer Ansicht (zum Beispiel `f` Filter) stehen in der Fußzeile der Auswahl. Wird die Fußzeile für das Terminal zu breit, bricht sie auf mehrere Zeilen um.

Bei Abbruch (Esc oder Ctrl+C) stellt Cubi den Cursor und den Terminal-Modus wieder her.

## Fenster

Hat eine Liste mehr Einträge, als in ein festes Fenster passen (höchstens 15 Zeilen, bei kleinen Terminals weniger), scrollt der Inhalt innerhalb dieses Fensters. Die Position steht in der Fußzeile (`12/57`).

## Mehrfachauswahl

In einer Mehrfachauswahl kommen drei Tasten dazu:

| Taste | Wirkung |
| --- | --- |
| Space | aktuellen Eintrag an- oder abwählen |
| `a` | alle Einträge an- oder abwählen (sind schon alle gewählt, werden alle abgewählt) |
| Enter | bestätigen |

Ausgewählte Einträge tragen `[x]`, die übrigen `[ ]`. Die Fußzeile zeigt die Anzahl (`3 gewählt`). Verlangt eine Auswahl mindestens einen Eintrag, bleibt Enter ohne Auswahl wirkungslos und die Meldung „Mindestens 1 Eintrag wählen“ erscheint.

```
? Apps: Auto-Sync deaktivieren
  4 Apps
      App    Projekt  Auto-Sync               Gemerkt
  [x] app-a  default  auto (prune, selfHeal)  -
❯ [x] app-b  default  manual                  auto (prune)
  [ ] app-c  default  auto                    -
  3 gewählt · ↑↓ Bewegen · Space Wählen · a Alle · Enter Bestätigen
```

nach Enter:

```
? Apps: Auto-Sync deaktivieren › 2 gewählt: app-a, app-b
```

## Wo es Mehrfachauswahl gibt

| Ort | Aktion |
| --- | --- |
| `cubi autosync` (auch im ArgoCD-Menü mit `s`) | Auto-Sync für mehrere Apps ändern |
| `cubi branch` (auch im ArgoCD-Menü mit `b`) | Branch mehrerer Apps umstellen |
| ArgoCD-App-Liste, `m` | mehrere Apps in einem Aufruf synchronisieren (vorgewählt: alle OutOfSync-Apps) |
| GitHub, Issue bearbeiten | mehrere Labels hinzufügen oder entfernen, mehrere Personen zuweisen oder entfernen |

Alle übrigen Auswahlen sind Einfachauswahlen, weil die Aktion nur für genau ein Element Sinn ergibt (Verbinden, Namespace/Kontext wechseln, Token rotieren, Pod-Shell und so weiter).

## Bestätigungen

Ändernde Aktionen fragen mit `(y/N)` nach. Die Voreinstellung steht in Großbuchstaben, Enter nimmt sie. `y`/`j` bestätigen, `n` und Esc lehnen ab.
