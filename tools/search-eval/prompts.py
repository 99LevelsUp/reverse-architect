"""The prompt layer of the experiment: the two templates and what fills them (R6, KTD8, KTD10).

A prompt is identical across variants word for word except for the paragraph about available
sources (`sources_paragraph`, R6), for a question as for a case, and is stored verbatim in the
report with its SHA-256 (KTD8 point 5). `run.py` re-exports every name of this module, so
`run.build_prompt` and the others keep working for their callers; the answer check
(`answers.py`) rebuilds a question's prompt from the stored template with `build_prompt`.
"""
from __future__ import annotations

from pathlib import Path

from catalog import Case, Question
from report import ROLE_DEVELOPER, ROLE_SUPPORT

PROMPT_PLACEHOLDER = "{zadani_pripadu}"
PATH_BASE = {ROLE_DEVELOPER: "relativně k pracovnímu adresáři",
             ROLE_SUPPORT: "relativně ke kořeni zdrojáků systému, jak ji uvádí korpus"}

PROMPT_TEMPLATE = """Řešíš případ, který přišel na podporu. Najdi v kódu místo, kde leží příčina \
chyby: soubor a člen (metodu, vlastnost nebo jiný člen typu). Kód neopravuj a nic neměň.

Zadání případu:
{zadani_pripadu}

Dostupné zdroje:
{zdroje}

Odevzdání: jakmile místo příčiny najdeš, skonči. Na konec své poslední zprávy dej právě \
jeden blok JSON v tomto tvaru:
```json
{{"file": "<cesta k souboru {vztah_cesty}>", \
"member": "<Typ.Člen>", "hypothesis": "<krátká hypotéza, proč tam je příčina>"}}
```
"""

# The prompt of a question (KTD10): the question, the same paragraph about sources, then a
# free-text answer that cites places as anchors in backticks. No JSON is asked for.
QUESTION_PROMPT_TEMPLATE = """Odpovídáš na otázku uživatele systému. Kód neopravuj a nic neměň.

Otázka:
{zadani_pripadu}

Dostupné zdroje:
{zdroje}

Odevzdání: odpověz volným textem a skonči. Místa v kódu, o která svou odpověď opíráš, cituj \
jako kotvy v backticku: `cesta:N` nebo `cesta:N-M`, kde cesta je {vztah_cesty} a N a M jsou \
čísla řádků. Žádný jiný tvar odpovědi se nevyžaduje, JSON nepiš.
"""


def sources_paragraph(corpus: Path | None, role: str) -> str:
    """The only part of the prompt that differs between variants (R6).

    The support role says the agent has no sources; the developer role is the
    wording of every earlier experiment."""
    if role == ROLE_SUPPORT:
        parts = ["Zdrojáky systému nemáš, pracovní adresář je prázdný."]
    else:
        parts = ["Zdrojáky systému jsou v pracovním adresáři (strom bez historie gitu)."]
    if corpus is None:
        parts.append("Jiné zdroje nemáš.")
    else:
        parts.append(f"Vedle nich máš korpus reverse-architect jako soubory v adresáři "
                     f"{corpus}: mapy, karty a jejich detaily, které popisují principy, záměry "
                     "a vazby a odkazují do kódu. Čteš je a hledáš v nich stejnými nástroji "
                     "jako ve zdrojácích.")
    return " ".join(parts)


def prompt_template(sources: str, role: str, druh: str = Case.druh) -> str:
    """A variant's prompt with the item's text left as a placeholder (for the report).

    `druh` is the kind of the item: a case (`chyba`, JSON answer) or a question (`otazka`,
    free answer with anchors)."""
    base = QUESTION_PROMPT_TEMPLATE if druh == Question.druh else PROMPT_TEMPLATE
    text = base.replace("{zdroje}", sources).replace("{vztah_cesty}", PATH_BASE[role])
    return text.replace("{{", "{").replace("}}", "}")


def item_text(item: Case | Question) -> str:
    """What the agent is given to work on: a case's prompt or a question's `dotaz`."""
    return (item.dotaz if isinstance(item, Question) else item.prompt).strip()


def build_prompt(template: str, item: Case | Question) -> str:
    return template.replace(PROMPT_PLACEHOLDER, item_text(item))
