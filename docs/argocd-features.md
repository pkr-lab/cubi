# ArgoCD-Features

Beide Features ändern die ArgoCD-`Application` über `kubectl patch`. Das ist derselbe Weg, über den Cubi die Applications schon liest (`kubectl get applications.argoproj.io -n <argocd.namespace>`). Es kommt kein neuer Zugriffsweg dazu. Der Kontext von `kubectl` muss auf den Cluster mit ArgoCD zeigen und Patch-Rechte auf `applications.argoproj.io` haben.

Aufruf:

```bash
cubi autosync     # oder: cubi sync
cubi branch
```

Beide stehen auch im Hauptmenü, und in der ArgoCD-App-Liste öffnen `s` (Auto-Sync) und `b` (Branch) sie direkt.

## Auto-Sync deaktivieren und aktivieren

1. Aktion wählen: *Auto-Sync deaktivieren* oder *Auto-Sync aktivieren*.
2. Apps per Mehrfachauswahl wählen. Die Liste zeigt pro App den aktuellen Zustand:

   | Spalte | Bedeutung |
   | --- | --- |
   | Auto-Sync | `auto (prune, selfHeal)` oder `manual` |
   | Gemerkt | die beim Deaktivieren gespeicherten Einstellungen, sonst `-` |

   Apps, für die die Aktion nichts ändern würde, erscheinen abgeblendet und werden übersprungen.
3. Cubi zeigt die geplanten Änderungen und fragt `(y/N)`.

**Deaktivieren** entfernt `spec.syncPolicy.automated` und legt die bisherigen Einstellungen als JSON in die Annotation `cubi.pkr-lab.io/previous-automated-sync`. Das umfasst `prune`, `selfHeal` und `allowEmpty`. Beides geschieht in einem einzigen Patch.

**Aktivieren** schreibt die gemerkten Einstellungen zurück nach `spec.syncPolicy.automated` und entfernt die Annotation. Gibt es keine gemerkten Einstellungen (oder sind sie nicht lesbar), setzt Cubi `automated` ohne `prune` und `selfHeal` und weist in der Zusammenfassung darauf hin.

Wird die App von einem ApplicationSet oder einer übergeordneten App verwaltet, warnt Cubi, weil diese die Änderung wieder zurücksetzen können.

Beispiel:

```
$ cubi autosync
? Auto-Sync › Auto-Sync deaktivieren
? Apps: Auto-Sync deaktivieren › 2 gewählt: app-a, app-c

──────────── Geplante Änderungen ────────────
  app-a  auto (prune, selfHeal) → manual
    Einstellungen werden gemerkt (prune, selfHeal)
  app-c  auto → manual
    Einstellungen werden gemerkt (ohne prune/selfHeal)
    wird von ApplicationSet apps verwaltet, die Änderung kann dort zurückgesetzt werden
  app-b  übersprungen (bereits manual)
? 2 App(s) ändern? › Ja
✔ app-a: Auto-Sync deaktiviert (manual)
✔ app-c: Auto-Sync deaktiviert (manual)
```

Später wieder einschalten:

```
$ cubi autosync
? Auto-Sync › Auto-Sync aktivieren
? Apps: Auto-Sync aktivieren › 1 gewählt: app-a
  app-a  manual → auto (prune, selfHeal)
    gemerkte Einstellungen werden wiederhergestellt (prune, selfHeal)
```

Von Hand nachsehen:

```bash
kubectl get application app-a -n argocd -o jsonpath='{.metadata.annotations.cubi\.pkr-lab\.io/previous-automated-sync}'
```

## Branch einer App umstellen

1. Modus wählen: *Branch umstellen* oder *Auf main zurücksetzen*.
2. Apps per Mehrfachauswahl wählen. Die Liste zeigt den aktuellen Branch (`spec.source.targetRevision`, `HEAD` wenn leer), den Auto-Sync-Zustand und das Quell-Repository. Bei Multi-Source-Apps stehen die Branches mit `|` getrennt in der Spalte, hinter dem Repository steht `(+n)`. Im Modus *Auf main zurücksetzen* sind alle Apps vorgewählt, die nicht auf `main` stehen.
3. Beim Umstellen fragt Cubi den Ziel-Branch ab (Standard `main`).
4. Bei Apps mit `spec.sources` wählen Sie pro App, welche Source gemeint ist.
5. Cubi prüft, ob der Branch im Remote-Repo existiert, zeigt die Zusammenfassung mit Hinweisen und fragt `(y/N)`.

Geändert wird nur die gewählte `targetRevision`. Der Patch ist ein JSON-Patch mit vorgeschaltetem `test` auf die `repoURL`. Hat sich die App zwischenzeitlich verändert, bricht der Patch ab, statt die falsche Source zu treffen.

**Existenzprüfung.** Bei GitHub-Repositories fragt Cubi über `gh api` nach Branches und Tags (auch für private Repos, soweit `gh` angemeldet ist). Bei anderen Remotes nutzt es `git ls-remote` ohne Passwortabfrage. Fehlt der Branch, steht in der Zusammenfassung `⚠ Branch … existiert nicht` und die Rückfrage bleibt bei Nein als Voreinstellung. Lässt sich das nicht prüfen (kein Zugriff, Commit-ID), steht `Existenz im Remote-Repo nicht prüfbar`. Umstellen lässt sich trotzdem.

**Hinweis bei Auto-Sync.** Ist Auto-Sync aktiv, weist Cubi darauf hin, dass ArgoCD sofort auf den neuen Branch synct. Ist zusätzlich `prune` aktiv, kommt die Warnung, dass Ressourcen, die es im neuen Branch nicht gibt, gelöscht werden.

**Zurücksetzen auf main.** *Auf main zurücksetzen* nutzt dieselbe Strecke mit festem Ziel `main`. Sie überspringt nur die Abfrage des Ziel-Branches. Alternativ genügt beim Umstellen Enter bei der Frage nach dem Ziel-Branch, da `main` vorbelegt ist.

Beispiel:

```
$ cubi branch
? Branch › Branch umstellen
? Apps: Branch umstellen › 2 gewählt: app-a, app-d
? Ziel-Branch › feature/login
? Source von app-d › pkr-lab/charts chart @ v1

──────────── Geplante Änderungen ────────────
  app-a  main → feature/login
    ⚠ Auto-Sync aktiv (prune, selfHeal): ArgoCD synct sofort auf den neuen Branch
    ⚠ prune aktiv: Ressourcen, die im neuen Branch fehlen, werden gelöscht
  app-d  v1 → feature/login  (pkr-lab/charts chart @ v1)
    ⚠ Branch feature/login existiert nicht in pkr-lab/charts
? 2 App(s) auf feature/login umstellen? › Nein
```

Zurücksetzen:

```
$ cubi branch
? Branch › Auf main zurücksetzen
? Apps: auf main zurücksetzen › 1 gewählt: app-a
  app-a  feature/login → main
? 1 App(s) auf main umstellen? › Ja
✔ app-a: feature/login → main
```

Hinweis: Eine App, die ArgoCD selbst über ein ApplicationSet oder App-of-Apps verwaltet, kann den Branch dort wieder überschrieben bekommen. Dauerhaft ändert man ihn in der Quelle der übergeordneten Ressource.
