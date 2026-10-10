# Měření vyhledávání (search-eval)

Běžeč srovnávacího pokusu: agent řeší vyřešené případy z podpory ve dvou variantách a zpráva ukáže, ve které z nich najde místo příčiny spolehlivěji, rychleji a levněji. Výchozí dvojice je „bez karet“ (jen zdrojáky) a „s kartami“ (zdrojáky a korpus reverse-architect). Stejně jde porovnat dvě varianty korpusu, například korpusy vygenerované s různými volbami RA.

Agent v obou variantách pracuje jen se soubory: zdrojáky v pracovním stromu, ve variantě s korpusem navíc soubory korpusu. Žádná varianta nemá server MCP ani jiný zdroj mimo tyto soubory.

Běžeč je Python se standardní knihovnou (3.12 nebo novější) a stojí mimo binárku RA. Jádro RA kvůli němu nemá žádnou úpravu.

## Co měření měří a co netvrdí

Měří agenta (Claude Code, pevný model a strop) v pracovním postupu se zdrojáky a korpusem RA čteným jako soubory, na konkrétních vyřešených případech. Každý případ běží v každé variantě třikrát čistě. Běh je úspěšný, když agent odevzdá soubor a člen z množiny přijatelných míst. Zpráva uvede pro každý případ párový rozdíl variant: počet úspěšných běhů, mediány tokenů (vstup bez cache, zápis do cache, čtení z cache, výstup), kroků a času, a výsledek výhra, prohra nebo remíza. Práh úspěchu nestanoví. Výsledek smí vyjít nerozhodně.

Měření netvrdí:

- nic o lidech, je to zástupná veličina za navigační část investigace;
- nic o příjmu případu ani o ověření verze, ve které chyba vznikla;
- že rozdíl způsobil samotný obsah karet, protože varianta s kartami měří korpus a způsob, jak v něm agent hledá, společně;
- nic o případech, jejichž zadání samo jmenuje místo příčiny. Zpráva je uvádí zvlášť, stejně jako případy použité při ladění karet.

Stejný oddíl nese i každá zpráva.

## Kde co leží

| Co | Kde |
|---|---|
| Kód běžeče, formát katalogu, anonymní ukázka | tady v repu (`examples/catalog.example.json`) |
| Skutečný katalog případů | mimo repo |
| Soubor variant, tarif | mimo repo |
| Export stromu, korpusy, kořen izolace, kořen výstupu | mimo repo i mimo zdrojové repo |
| Soubor hodnotitelů (ukázka `examples/judges.example.json`), stavový adresář | mimo repo i mimo zdrojové repo |
| Soubory kontroly vlastníka (JSON i `.md`), výstup zprávy odpovědí | mimo repo i mimo zdrojové repo |
| Kalibrační sada (ukázka `examples/calibration.example.json`), výsledek kalibrace | mimo repo i mimo zdrojové repo |

Případy nesou jména zákazníků, popisy chyb a přílohy ze systému podpory, tedy citlivá data zákazníků. Proto katalog případů, přepisy běhů ani zprávy do repa nepatří. Běžeč kořen výstupu, export, kořen izolace i korpusy převede na skutečnou cestu (včetně symlinků a junctions) a cestu uvnitř tohoto repa, zdrojového repa nebo kteréhokoli worktree jednoho z nich odmítne (kód 2, chyba cestu jmenuje). Stejně odmítne katalog, a to dřív, než ho přečte. `answers.py` totéž ověří u katalogu, souboru hodnotitelů, stavového adresáře, souborů kontroly (JSON, `.md` vedle něj i soubor návrhů), kalibrační sady, výsledku kalibrace a výstupu zprávy, a to před čtením nebo zápisem čehokoli. Stavový adresář (cache hodnotitele, měření, rozhodnutí) se navíc nesmí překrývat s korpusem, který příkaz dostal (`--korpus`, nebo korpus kterékoli varianty pokusu předaného do `hodnot-odpovedi`). Kořen výstupu a katalog nesmí ležet ani v exportu nebo v korpusu, protože ty agent čte. Volba `--forbid-dir JMÉNO` (lze opakovat) navíc odmítne každou z těchto cest, která vede přes adresář toho jména (bez ohledu na velikost písmen), například přes pracovní prostor jiného nástroje, který agent nemá vidět.

## Postup operátora

### 1. Základní revize a případy

1. Vyber jednu základní revizi zdrojového repozitáře gitu. Všechny případy běží nad ní.
2. Do katalogu dej jen případy opravené až po ní, jejichž chyba v ní existuje. Běžeč odmítne případ, jehož commit opravy není potomkem základní revize, nebo jehož přijatelný člen v exportu základní revize neexistuje.
3. Přijatelná místa příčiny odvoď z commitu opravy a projdi je ručně, ne podle karty RA. Když oprava zasáhla víc míst, uveď všechna, která uznáváš. Případ, u kterého místo nejde spolehlivě určit, do pokusu nedávej.
4. U každého případu ručně ověř, že chyba v základní revizi existuje a přijatelný člen v ní je, a teprve pak nastav `"bug_confirmed_at_base": true`.
5. Označ případy použité při návrhu a ladění karet (`used_for_card_tuning`) a případy, jejichž zadání samo jmenuje místo příčiny (`prompt_names_location`).

### 2. Pseudonymizace zadání

Zadání se před uložením do katalogu pseudonymizuje. Data zákazníků k poskytovateli modelu nejdou.

- Jména zákazníků, firem a osob a jejich identifikátory (IČO, e-maily, telefony, jména databází a serverů, adresy) nahraď zástupnými názvy: „Zákazník A“, „Firma X“, „Osoba 1“. Stejné jméno dostane v celém zadání stejný zástupný název.
- Identifikátory ze zdrojového kódu (třídy, metody, hlášky) ponech beze změny. Kód není dato zákazníka a agent podle nich hledá.
- Pseudonymizuj ručně nebo lokálním nástrojem. Nepseudonymizovaný text neposílej žádnému modelu.
- Ručně zkontroluj, že text dál vede k místu příčiny, a teprve pak nastav `"pseudonymization_confirmed": true`. Běžeč případ bez obou potvrzení odmítne.

### 3. Katalog

Formát popisuje docstring `catalog.py`, vzor je v `examples/catalog.example.json`. Všechna pole jsou povinná:

```json
{
  "format_version": 1,
  "base_revision": "<sha základní revize>",
  "cases": [
    {
      "id": "<identifikátor případu>",
      "prompt": "<pseudonymizované zadání, jak ho dostala podpora>",
      "acceptable_locations": [
        {"file": "<cesta relativně ke kořeni repa, oddělovač />",
         "member": "<Typ.Člen>",
         "line": 120,
         "fix_commit": "<sha commitu opravy>"}
      ],
      "bug_confirmed_at_base": true,
      "pseudonymization_confirmed": true,
      "used_for_card_tuning": false,
      "prompt_names_location": false
    }
  ]
}
```

Přetížení člena smí katalog zapsat v notaci indexu SCIP (`Rounding.Apply(+1)`). Shoda odpovědi se posuzuje jen podle kvalifikovaného jména, přetížení se nerozlišuje (zdůvodnění v docstringu `score.py`). Katalog ulož mimo repo do vlastního adresáře, například `<adresář pokusu>\katalog\katalog.json`. Adresář katalogu nesmí obsahovat kořen izolace: izolovaný adresář nastavení agenta musí ležet mimo něj, jinak kontrola nástroje agenta nespustí.

### 4. Varianty a korpus s účtenkou

Bez souboru variant porovná běžeč dvojici „bez karet“ a „s kartami“. Korpus varianty s kartami určuje `--corpus-dir`, volby RA pro něj `--ra-option` (lze opakovat). Jinou dvojici, například dva korpusy, zadá soubor `--variants`:

```json
{
  "format_version": 1,
  "variants": [
    {"name": "korpus A", "corpus_dir": "korpus-a", "ra_options": []},
    {"name": "korpus B", "corpus_dir": "korpus-b", "ra_options": ["<volba RA>"]}
  ]
}
```

Pokus porovnává právě dvě varianty s různými jmény. První je referenční, druhá srovnávaná. Varianta bez korpusu má `"corpus_dir": null` nebo pole nemá, volby RA bez korpusu běžeč odmítne. Nepovinné pole `card_format` jen popisuje formát karet ve zprávě. Relativní cesty v souboru variant se berou od souboru.

Nepovinné pole `ra_command` určuje příkaz RA jen pro korpus té varianty: neprázdný seznam textů, binárka a její pevné argumenty (například `["<jiná binárka RA>"]`), vzatý tak, jak je zapsaný. Bez něj korpus vygeneruje `--ra-binary`. Tak jde porovnat dvě verze RA: každá varianta má vlastní příkaz a verzi RA si běžeč zapíše do účtenky a do zprávy. Příkaz RA bez korpusu běžeč odmítne.

Korpus každé varianty vygeneruje běžeč sám binárkou RA z exportu základní revize a vedle korpusu zapíše účtenku `<korpus>.receipt.json`: revizi, otisk stromu, verzi RA, volby a otisk korpusu. Korpus bez platné účtenky, z jiného stromu nebo změněný po vystavení účtenky běžeč odmítne. Agent dostane korpus jen jako adresář se soubory, které čte a prohledává stejnými nástroji jako zdrojáky. Každá varianta vidí jen svůj korpus, čtení cizího korpusu běh zkazí.

Korpus jde připravit předem: `--prepare-only` provede export, kontrolu případů a vygeneruje korpusy s účtenkou, pak skončí. Agenta nespustí. Korpus si tak před pokusem prohlédneš.

Další spuštění potřebuje nový prázdný `--export-dir`. Korpus se znovu použije podle účtenky, pokud jsou volby RA stejné jako při generování.

#### Role čtenáře (`--role`)

`--role` je povinná a nemá výchozí hodnotu, bez ní běžeč neběží.

- `vyvojar`: agent je čtenář se zdrojáky. Pracuje v exportu základní revize, platí výchozí dvojice variant. Takhle běžel každý dosavadní pokus.
- `podpora`: agent je čtenář bez zdrojáků. Jeho pracovní adresář je prázdný izolovaný adresář (vedle `--export-dir`, pojmenovaný podle něj s příponou `-podpora-prazdny`, musí neexistovat nebo být prázdný) a korpus čte přes `--add-dir`. Zadání říká, že zdrojáky nemá, a audit uznává jen korpus: čtení souboru ze zdrojáků je `path_outside` a běh se zkazí. Kontrola nástroje pro tuto roli nevytváří pracovní adresář se zdrojáky.

V roli `podpora` se položky druhu `chyba` neměří. Zpráva je uvede jako „v roli podpora se neměří“. Katalog bez jiné položky než `chyba` běžeč odmítne (kód 2). Výchozí dvojice variant tu nedává smysl („bez karet“ nemá co číst), proto se obě varianty zadávají výslovně souborem `--variants`, každá s korpusem. Výchozí volání i varianta bez korpusu skončí kódem 2. Role je ve zprávě, v `report.json` jako `experiment.role` a seznam neměřených položek jako `experiment.not_measured`.

### 5. Spuštění z externího terminálu

Běžeč spouštěj z externího terminálu, ne z relace Claude Code. Uvnitř relace Claude Code se `claude` spouštět nesmí: brání tomu ochrana `CLAUDECODE` a běh by navíc stál uvnitř cizí relace a jejího nastavení. Běžeč sám dítěti předá jen proměnné ze seznamu povolených, žádné `CLAUDE*` ani `ANTHROPIC*` z rodiče.

Agent se přihlašuje kopií přihlášení z předplatného (výchozí `~/.claude/.credentials.json`, jinak `--credentials`). Kopie vzniká v izolovaném adresáři nastavení pod `--isolation-root`, s právy jen pro uživatele, a po každém běhu se smaže, i po stropu, výjimce nebo Ctrl+C. Když běžeč při startu najde zbytek po předchozím běhu, odmítne pokračovat. Zbytek ověř a smaž ručně.

Riziko: CLI může v kopii obnovit token. U OAuth přihlášení to může zneplatnit token hlavního přihlášení. Po pokusu proto ověř, že hlavní přihlášení funguje (spusť `claude` v běžném terminálu, případně se znovu přihlas přes `/login`).

#### Suchý běh

```powershell
python tools\search-eval\run.py `
  --catalog <adresář pokusu>\katalog\katalog.json `
  --source-repo <zdrojový repozitář gitu> `
  --output-root <adresář pokusu>\vystupy `
  --export-dir <adresář pokusu>\export-1 `
  --isolation-root <adresář pokusu>\izolace `
  --ra-binary <cesta k binárce reverse-architect> `
  --model <model agenta> `
  --cap-seconds <s> --cap-steps <kroky> --cap-tokens <tokeny> `
  --seed <semínko> `
  --role <vyvojar|podpora> `
  --corpus-dir <adresář pokusu>\korpus-s-kartami `
  --dry-run
```

Adresář pokusu musí ležet mimo tento repozitář i mimo zdrojový repozitář. `--dry-run` ověří vstupy a cesty a vypíše počet běhů, nejvyšší počet s náhradami a strop. Agenta nespustí a nic nezapíše.

Model, nastavení a strop (čas, kroky, tokeny) jsou pro celý pokus pevné a stejné pro obě varianty. Strop tokenů platí pro součet čtyř kategorií. Běžeč ho vynucuje sám a při překročení ukončí agenta i s potomky. Zpráva u takového běhu počítá s hodnotou stropu. `--tariff` (JSON s `currency`, `per_million` po kategoriích a `source`) přidá do zprávy peníze. Bez tarifu je zpráva neuvádí.

#### Kontrola nástroje a pokus

Stejný příkaz bez `--dry-run` spustí pokus. Před prvním během proběhne pro každou variantu kontrola nástroje (KTD10) na syntetických značkách. Kontrola ověří, že CLI odepře čtení mimo pracovní strom, nenačte uživatelské instrukce, nenabídne jiné nástroje než Read, Grep a Glob ani žádný server MCP, že přepis nese telemetrii a že kopie přihlášení funguje. Když kterákoli kontrola neprojde, pokus nezačne.

Běhy jdou po jednom v pořadí zamíchaném podle semínka. Zkažený běh, tedy běh s pokusem o zakázaný přístup (nástroj mimo Read, Grep a Glob, cesta mimo pracovní strom a korpus varianty, git, adresář nastavení agenta), se nahradí nejvýš dvakrát na případ a variantu. Případ bez tří čistých běhů je ve zprávě neúplný. Chyba infrastruktury (přihlášení, výpadek API, limit) pokus zastaví a do výsledků se nepočítá.

### 6. Kódy ukončení

| Kód | Význam |
|---|---|
| 0 | pokus dokončen, nebo jen suchý běh či příprava (`--prepare-only`) |
| 2 | neplatný vstup nebo příprava vstup odmítla (katalog, varianty, cesty, korpus) |
| 3 | chyba infrastruktury: kontrola nástroje neprošla nebo běh selhal mimo agenta |
| 130 | pokus přerušil operátor (Ctrl+C); částečná zpráva zůstane |

### 7. Výstupy

Každé spuštění založí pod `--output-root` adresář `pokus-<datum>-<čas>-seed<semínko>/`:

| Soubor | Obsah |
|---|---|
| `report.md`, `report.json` | zpráva se stejnými čísly v obou souborech |
| `preparation.json` | revize, otisky, účtenky korpusů, náklady přípravy |
| `preflight.json`, `preflight/` | kontrola nástroje po variantách |
| `schedule.json` | semínko, plánované a provedené pořadí běhů, náhrady |
| `prompts/` | doslovná zadání pro každý případ a variantu |
| `runs/` | přepis (`.jsonl`) a výsledek (`.result.json`) každého běhu |

Zadání a přepisy obsahují pseudonymizovaná data případů. Adresář pokusu nech mimo repo a nesdílej ho mimo okruh, který smí případy vidět.

### 8. Místo na disku

Korpus velkého zdrojového repozitáře může mít jednotky GB a každá varianta má vlastní. K tomu se přičte export stromu, který potřebuje nový prázdný adresář při každém spuštění. Před pokusem uvolni místo, staré exporty a korpusy smaž.

### 9. Po pokusu

1. Ověř, že hlavní přihlášení Claude Code funguje.
2. Zprávu a artefakty ponech mimo repo.

## Otázky a kontrola odpovědí

Vedle případů (`chyba`) nese katalog otázky (`otazka`). Otázka je dotaz uživatele systému s ověřitelnými fakty a se známými nepravdami. Agent na ni odpoví volným textem a místa v kódu cituje jako kotvy v backticku (`cesta:N` nebo `cesta:N-M`). Odpověď se neskóruje jako případ, hodnotí se proti faktům otázky. Tutéž kontrolu faktů jde pustit i nad samotným korpusem, bez agenta. Přehled příkazů:

| Příkaz | Co dělá |
|---|---|
| `answers.py kontrola-karet` | ověří, že karty korpusu uvádějí fakta otázek a neuvádějí zakázaná tvrzení (bez agenta) |
| `run.py --role …` | agent odpovídá na otázky v obou rolích, odpověď se uloží |
| `answers.py hodnot-odpovedi` | ohodnotí uložené odpovědi z pokusu proti faktům a zakázaným tvrzením |
| `answers.py kontrola` | soubor ke kontrole vlastníka (sporné jednotky a vzorek) a načtení jeho rozhodnutí |
| `answers.py zprava` | zpráva odpovědí: fakta a zakázaná tvrzení po otázkách, cestách a korpusech |
| `answers.py kalibrace` | kalibrace hodnotitele na zmrazené sadě vět |

Každý podpříkaz `answers.py` má povinné `--source-repo` (zdrojový repozitář, podle něj se určují chráněné kořeny, viz „Kde co leží“). Žádná cesta nemá výchozí hodnotu. Kódy ukončení: 0 hotovo, 2 neplatný vstup (i chybějící argument nebo cesta uvnitř chráněného kořene), 3 hodnotitel nebo infrastruktura selhaly (měření zůstane `nedokonceno` a stejný příkaz pokračuje z cache hodnotitele), 130 přerušeno (Ctrl+C).

### Sestavení otázky

1. Základní revize je zafixovaná: otázka nese `commit`, který se rovná `base_revision` katalogu, a všechna fakta se odvozují ze zdrojáků v něm.
2. Claude navrhne fakta nad zdrojáky v tomto commitu: každý fakt je jedna ověřitelná věta s místem (soubor, člen nebo typ, řádky), s rolí `ramcova` (patří do rámcové odpovědi pro čtenáře bez zdrojáků) nebo `misto` (místo pro vývojáře). K tomu navrhne zakázaná tvrzení, tedy známé nepravdy, které se u dané otázky nabízejí, s místy, která je vyvracejí. Návrh vzniká ze zdrojáků, ne z karet korpusu, jinak by měření ověřovalo karty jejich vlastním obsahem.
3. Vlastník každý fakt i zakázané tvrzení projde v kódu, ověří místo a řádky a zadání dotazu pseudonymizuje (viz „Pseudonymizace zadání“). Teprve pak nastaví `fakta_potvrzena` a `pseudonymizace_potvrzena` na `true`.
4. Při potvrzení se spočítá otisk potvrzení a zapíše do `otisk_potvrzeni`. Je to `confirmation_fingerprint` z `catalog.py`: SHA-256 kanonického JSON z polí `fakta` a `zakazana_tvrzeni` tak, jak stojí v souboru. Fakt nebo tvrzení upravené po potvrzení otisk změní a běžeč katalog odmítne, dokud vlastník fakta znovu nepotvrdí. Otisk spočítá tento skript (uložený mimo repo), argumenty jsou katalog a `id` otázky:

```python
import json, sys
sys.path.insert(0, "tools/search-eval")
from catalog import confirmation_fingerprint

katalog, otazka = sys.argv[1], sys.argv[2]
for polozka in json.load(open(katalog, encoding="utf-8"))["cases"]:
    if polozka.get("id") == otazka:
        print(confirmation_fingerprint(polozka["fakta"], polozka["zakazana_tvrzeni"]))
```

Úplný formát otázky popisuje docstring `catalog.py`, vzor je v `examples/catalog.example.json` (zástupná jména `ViewA`, `ModelB`, `AcmeState`).

### Kontrolní otázky

Kontrolní otázka (`"kontrolni": true`) není přejímací položka. Slouží k tomu, aby se ukázalo, že karty a postupy nejsou vyladěné jen na otázky, nad kterými vznikaly. Zpráva je uvádí zvlášť. Proto platí:

- Kontrolní otázky leží v katalogu mimo repo, stejně jako ostatní otázky, a nikde jinde. Do repa, do dokumentů v repu ani do commitů nepatří.
- Nesdílí se s jinými sessions. Session, která ladí karty, extraktor nebo plugin, je nezná a nedostane je v zadání, v souboru ani ve výpisu.
- Navrhuje je session, která krok 2a neimplementuje. Session implementace katalog nečte.

### Kontrola karet

```powershell
python tools\search-eval\answers.py kontrola-karet `
  --source-repo <zdrojový repozitář gitu> `
  --agent-family <rodina agenta, například claude> `
  --katalog <adresář měření>\katalog\katalog.json `
  --korpus <adresář korpusu> `
  --hodnotitele <adresář měření>\hodnotitele\judges.json `
  --stav <adresář měření>\stav-korpus-a
```

Příkaz je bez agenta: soubory korpusu čte přes index a hodnotitel (model, který nastavuje vlastník, viz „Soubor hodnotitelů“) posuzuje, zda dokument uvádí fakt nebo obsahuje zakázané tvrzení. Účtenka korpusu není potřeba. Revize korpusu se musí rovnat commitu otázky, jinak příkaz skončí kódem 2 dřív, než se hodnotitel spustí. Výsledek je `measurement.json` ve stavovém adresáři, jeden korpus na jedno měření. Měření jiného vstupu ve stejném stavovém adresáři se uloží vedle jako `measurement.<id>.json`. Hodnotitel se vybírá z jiné rodiny, než je rodina agenta (`--agent-family`).

### Běh v obou rolích

Otázky běží v pokusu `run.py` v obou rolích: `vyvojar` (agent má zdrojáky) a `podpora` (agent zdrojáky nemá a čte jen korpus). Roli určuje povinná volba `--role`, viz „Role čtenáře“ výše. V roli `podpora` jsou otázky jediné měřené položky, takže katalog bez otázky běžeč odmítne. Odpověď agenta (jeho poslední zpráva) se uloží do výsledku běhu jako `answer_text` a zpráva pokusu ji nehodnotí, jen počítá běhy.

### Hodnocení odpovědí

```powershell
python tools\search-eval\answers.py hodnot-odpovedi <adresář pokusu> `
  --source-repo <zdrojový repozitář gitu> `
  --katalog <adresář měření>\katalog\katalog.json `
  --hodnotitele <adresář měření>\hodnotitele\judges.json `
  --stav <adresář měření>\stav-pokus
```

Příkaz čte adresář pokusu z `run.py` (`report.json` a `runs/`) a hodnotí první tři uložené běhy každé otázky v každé variantě. Odmítne pokus, jehož otázka v katalogu chybí nebo je v něm nad jinou revizí, a pokus, jehož zadání otázky se od běhu změnilo. Pokus, který nemá stav `completed`, přijme jen s `--i-nedokonceny` a měření to vyznačí. Běh bez odpovědi (strop, chyba) se neposuzuje a v měření je uveden zvlášť. Výsledek je `<stav>\odpovedi\<pokus>\measurement.json`. Stavový adresář se nesmí překrývat s korpusem žádné varianty pokusu.

### Soubor hodnotitelů

Hodnotitel je příkazový řádek, který nastavuje vlastník (ukázka `examples/judges.example.json`, formát 1). Každá položka má `id`, `rodinu`, `prikaz` (seznam textů, text se posílá na standardní vstup), `model`, nepovinný `vystup` (`claude-json` nebo `codex-jsonl`), `prihlaseni` (soubory, jejichž kopii hodnotitel dostane) a `promenne` (jména proměnných prostředí, které se předají). Pořadí v souboru je pořadí preference. Soubor leží mimo repo, protože ukazuje na soubory s přihlášením. Příkazový řádek se nikdy nezapisuje do měření ani do zprávy, zapisuje se jen `id`, rodina a model, který CLI samo ohlásilo.

Hodnotitel běží v izolovaném adresáři pod stavovým adresářem (`<stav>\izolace`): vlastní adresář nastavení s kopií přihlášení, prostředí ze seznamu povolených proměnných, prázdný pracovní adresář. Po běhu se smaže, i po chybě. Před měřením projde zkouškou (platný JSON a soubor podstrčený vedle pracovního adresáře nesmí přečíst).

### Kontrola vlastníka

Než vznikne zpráva, vlastník projde všechny sporné jednotky a náhodný vzorek bezesporných:

```powershell
python tools\search-eval\answers.py kontrola `
  --source-repo <zdrojový repozitář gitu> `
  --stav <adresář měření>\stav-korpus-a `
  --mereni <adresář měření>\stav-korpus-a\measurement.json `
  --vystup <adresář měření>\kontrola\kontrola-a.json
```

Příkaz zapíše soubor JSON k vyplnění a vedle něj `.md` ke čtení se stejnými čísly řádků. Existující soubor nepřepíše. Vzorek se losuje podle semínka (`--seminko`), které se zapíše do souboru. Vlastník do každého řádku vyplní pole `rozhodnuti` a soubor načte zpátky:

```powershell
python tools\search-eval\answers.py kontrola `
  --source-repo <zdrojový repozitář gitu> `
  --stav <adresář měření>\stav-korpus-a `
  --nacti <adresář měření>\kontrola\kontrola-a.json
```

`--nacti` uloží rozhodnutí do `rozhodnuti.json` ve stavovém adresáři a návrhy zakázaných tvrzení (pole `nepravda`) do souboru vedle kontrolního souboru. Katalog se nemění. Částečně vyplněný soubor jde načíst opakovaně. Rozhodnuté jednotky se podruhé nenabízejí.

### Zpráva

```powershell
python tools\search-eval\answers.py zprava `
  --source-repo <zdrojový repozitář gitu> `
  --mereni <měření korpusu A> --mereni <měření korpusu B> --mereni <měření odpovědí> `
  --vystup <adresář měření>\zprava `
  --stav <adresář měření>\stav-korpus-a `
  --kalibrace <adresář měření>\kalibrace
```

Zpráva dá vedle sebe kontrolu karet a běhy s agentem a zapíše `answers-report.json` a `answers-report.md` do výstupního adresáře. Odmítne vzniknout, dokud nějaká sporná nebo vylosovaná jednotka čeká na rozhodnutí vlastníka, a odmítne porovnávat vstupy nad různými revizemi, s různými otisky potvrzení nebo posouzené jiným hodnotitelem, modelem či verzí promptu. U každého posouzení říká, zda ho udělal model, nebo rozhodl vlastník. Kalibraci hodnotitele pro dvojici hodnotitel a verze promptu uvede, nebo uvede, že chybí. Volba `--sada-kalibrace` zúží hledání na kalibraci nad touto sadou.

### Kalibrace

```powershell
python tools\search-eval\answers.py kalibrace `
  --source-repo <zdrojový repozitář gitu> `
  --agent-family <rodina agenta> `
  --sada <adresář měření>\kalibrace-sada\sada.json `
  --hodnotitele <adresář měření>\hodnotitele\judges.json `
  --stav <adresář měření>\stav-korpus-a `
  --vystup <adresář měření>\kalibrace\hodnotitel-a.json
```

Sadu vět s očekávanými štítky píše vlastník (formát 1, ukázka `examples/calibration.example.json`, čtyři třídy: `vyjadruje`, `pravdiva_neodpovida`, `parafraze_nepravdy`, `popreni`). Každá věta se posoudí stejným protokolem a se stejnou cache jako měření. Výsledek je matice po třídách, prostá shoda, dvě nebezpečné buňky zvlášť (neodpovídající pravdivá věta uznaná za fakt, přehlédnutá parafráze nepravdy) a pokrytí. Kappa se nepočítá: při malém vzorku by zavádělo. Výsledek nese `id` hodnotitele, jeho ohlášený model, verzi promptu a otisk sady, takže zpráva najde kalibraci jen pro stejnou dvojici a stejnou sadu. Adresář, do kterého míří `--vystup`, předá `zprava` volbou `--kalibrace`. Výstup uvnitř repa příkaz odmítne dřív, než se hodnotitel spustí.

## Postup pro první měření

Postup drží pohromadě tři oddělení: kdo píše fakta, kdo implementuje a kdo posuzuje.

1. Fakta kontrolní otázky navrhuje session, která krok 2a neimplementuje. Navrhuje je nad zdrojáky v zafixované základní revizi, bez pohledu do karet. Vlastník je projde, pseudonymizuje dotaz a potvrdí, pak se spočítá otisk potvrzení (viz „Sestavení otázky“).
2. Session implementace katalog nečte. Dostane jen formát a příkazy z tohoto README.
3. Session, která kontrolní otázku navrhla, o jejím obsahu nic nezapisuje do diáře paměti, do auto-memory ani do poznámek: ani dotaz, ani místa v kódu, ani fakta. Zaznamená jen to, že otázka vznikla.
4. Katalog, soubor hodnotitelů, stavový adresář, kontrolní soubory, kalibrační sadu a výstupy ulož mimo repo a mimo zdrojové repo. Příkazy jiné cesty odmítnou.
5. Pořadí: kalibrace hodnotitele (`kalibrace`), kontrola karet nad každým korpusem (`kontrola-karet`), pokus v obou rolích (`run.py --role vyvojar`, `run.py --role podpora`), hodnocení odpovědí (`hodnot-odpovedi`) pro každý pokus, kontrola vlastníka (`kontrola`, pak `kontrola --nacti`) a nakonec zpráva (`zprava`).

## Testy běžeče

Testy běží bez sítě a bez agenta. Přehrávají skriptované přepisy nad malým testovacím repem v `tests/fixtures/e2e/`:

```sh
python -m unittest discover -s tools/search-eval/tests -t tools/search-eval
```

V CI je spouští job `search-eval` na Linuxu, macOS a Windows.
