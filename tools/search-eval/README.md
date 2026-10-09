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

Případy nesou jména zákazníků, popisy chyb a přílohy ze systému podpory, tedy citlivá data zákazníků. Proto katalog případů, přepisy běhů ani zprávy do repa nepatří. Běžeč kořen výstupu, export, kořen izolace i korpusy převede na skutečnou cestu (včetně symlinků a junctions) a cestu uvnitř tohoto repa nebo zdrojového repa odmítne. Kořen výstupu a katalog nesmí ležet ani v exportu nebo v korpusu, protože ty agent čte. Volba `--forbid-dir JMÉNO` (lze opakovat) navíc odmítne každou z těchto cest, která vede přes adresář toho jména (bez ohledu na velikost písmen), například přes pracovní prostor jiného nástroje, který agent nemá vidět.

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

## Testy běžeče

Testy běží bez sítě a bez agenta. Přehrávají skriptované přepisy nad malým testovacím repem v `tests/fixtures/e2e/`:

```sh
python -m unittest discover -s tools/search-eval/tests -t tools/search-eval
```

V CI je spouští job `search-eval` na Linuxu, macOS a Windows.
