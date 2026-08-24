# html5-parser

Rust-Crate: eigenständiger WHATWG-HTML5-Tokenizer und Tree-Construction.
Konzept & Herkunft: `README.md`. Umsetzungsplan: `plan/`.

`plan/` ist bewusst per `.gitignore` von diesem Repo ausgeschlossen —
lokale Arbeitsnotizen für die Entwicklung (Phasenpläne, Status,
Entscheidungs-Log), kein Teil des veröffentlichten Codes. Nach einem
frischen Clone existiert `plan/` deshalb nicht automatisch. Die für das
Verständnis des Crates wesentlichen Scope- und Architektur-Punkte stehen
deshalb redundant auch hier in `CLAUDE.md` und in `README.md`, nicht nur
in `plan/`; Detail-Rationale und der laufende Phasen-Status sind es
nicht und gehen bei fehlendem `plan/` verloren — das ist hingenommen,
kein Versehen.

Schwesterprojekt von [`html-conform`](../html-conform) (künftiger Ersatz für
dessen aktuelle externe HTML5-Parsing-Abhängigkeit, Schicht 1). Siehe
dessen `plan/DECISIONS.md` für Herkunft und die zweistufige
Scope-Entscheidung.

## Scope — zweistufig, nicht spekulativ generisch von Anfang an

1. **Schritt 1:** Nur das bauen, was `html-conform` für den Ersatz von
   Schicht 1 tatsächlich braucht — Tokenizer + Tree-Construction, deren
   Ausgabe `html-conform`s `src/infoset.rs::normalize()` direkt bedienen
   kann, inklusive Source-Positionen pro Knoten. Keine generische
   öffentliche API, kein Anspruch auf Wiederverwendbarkeit außerhalb dieses
   Zwecks in dieser Stufe.
2. **Schritt 2:** Erst nachdem Schritt 1 an `html-conform`s echtem Bedarf
   steht, den darin als eigenständig wiederverwendbar erkannten Teil
   (generischer WHATWG-HTML5-Tokenizer/Tree-Builder ohne
   `html-conform`-spezifisches Wissen) zur öffentlichen API dieses Crates
   machen.

## Architektur (Arbeitstitel)

```
HTML-Input (String) → tokenizer (WHATWG-Tokenizer-Zustandsautomat)
                     → tree_builder (WHATWG-Tree-Construction-Algorithmus,
                       inkl. Foreign Content / SVG / MathML)
                     → document (Element-/Text-/Kommentar-Baum mit Positionen)
```

## Arbeitsweise

Falls `plan/` lokal vorhanden ist (siehe Hinweis oben — nach einem
frischen Clone erst nicht):

- Aktueller Stand & nächster Schritt: `plan/00-STATUS.md`.
- Phasenpläne mit Schritten/Exit-Kriterien: `plan/0N-*.md`. Vor größeren
  Änderungen die passende Phase lesen, nicht am Plan vorbei arbeiten.
- Getroffene Entscheidungen: `plan/DECISIONS.md` — dort nachschlagen, bevor
  offene Fragen neu aufgerollt werden.

Fehlt `plan/`, bei Bedarf neu anlegen (Konventionen siehe bestehende
Commits/Kommentare im Code) statt zu warten, bis es wieder auftaucht.

## Feste Regeln

- Lizenz: **MIT**, von Anfang an (`Cargo.toml`: `license = "MIT"`).
- Normative Grundlage ist die
  [WHATWG-HTML5-Parsing-Spezifikation](https://html.spec.whatwg.org/multipage/parsing.html).
  Bei Unklarheiten dort nachschlagen, nicht aus anderen Implementierungen
  (z. B. `html5ever`) raten oder Code übernehmen.
- Scope bleibt in Schritt 1 an `html-conform`s tatsächlichem Bedarf
  ausgerichtet — keine verfrühte Generalisierung, siehe "Scope" oben.
- Kein `unsafe` ohne expliziten Grund und Kommentar.

## Definition of Done

Siehe "Exit-Kriterien" in der jeweiligen `plan/0N-*.md`-Datei, falls
vorhanden (siehe "Arbeitsweise" oben) — nicht global definiert, sondern
pro Phase.
