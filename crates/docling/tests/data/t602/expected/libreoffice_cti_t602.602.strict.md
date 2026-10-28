```
┌───────────────────────────────────────────────────────────────┐
│          Textový  editor Text602          │
│                                                               │
│                           verze: 3.00                         │
│                                                               │
│                    Software602, spol. s r.o.                  │
│                                                               │
│                          Pod pramenem 3,                      │
│                          Praha 4, Nusle                       │
│                                                               │
│  Objednávky ................. P.O.Box 1, 140 00  Praha 4      │
│  Prodejny :                                                   │
│               Praha 1, Spálená 19     (Po-Pá  9,00-17,00)     │
│               Praha 1, Biskupský Dvůr (Po-Pá 10,00-18,00)     │
│                                                               │
│  Ústředna :    (02)423817      nebo           (02)421368      │
│                                                               │
│  Hotline a záznam hovorů v mimopracovní době                  │
│                                                               │
│                (02)422506      nebo           (02)420905      │
│                                                               │
│  Fax :         (02)421105      Mail602 :      (02)420448      │
│                                                               │
│  Expedice :    (02)421517                                     │
│                                                               │
└───────────────────────────────────────────────────────────────┘
```

Textový editor Text602 verze 3.00 je součástí dvou obchodních produktů:

- Text602 verze 3.00
- Text602 KOMPLET

Doplňující komponenty KOMPLETU jsou popsány v kapitolách informačního souboru CTI_KOMP.602. Součástí standardní dodávky jsou vždy tyto soubory:

o **T602.EXE**, **T602.KLV** Exekuční soubor editoru **Text602** a připravené klávesnice KL8 a KL9 pro případnou redefinici.

o **T602.HLP** Soubor s nápovědou pro Text602.

o ***.FNT** Soubory fontů pro standardní grafické drivery devíti a čtyřiadvacetijehličkových maticových tiskáren v kvalitě: DRAFT, NLQ, LQ a pro tryskové a laserové tiskárny.

o ***.DST** Definiční soubory pro tiskárny. Podrobný přehled a popis vlastností DST souborů najdete v informačním souboru CTI_MATD.602.

o **PREVIEW.DST** Definiční soubor pro tisk zrcadlového obrazu tiskové stránky (A4) na obrazovku.

o **FAX.DST** Definiční soubor pro výstup do faxového souboru.

o **SLOV.MNU** Soubor pro slovenská menu (aktivuje se příkazem T602 /SLOV.MNU).

o **HC602.COM**, **PCXTF602.EXE**, **LOGO602.TIF** Program na snímání obsahu obrazovky a ukládání do souborů ve formátu TIFF. Převodní program pro obrázky uložené ve formátu PCX (např. z programu PaintBrush (Windows)) do formátu TIFF. Jako vzor je přiložen soubor LOGO602.TIF, který obsahuje znak naší firmy

Všechny soubory jsou dodávány v komprimované podobě a je nutné je instalovat pomocí programu INST602 (viz soubor CTI_1111.602).

Změny a doplňky k manuálu

- spouštěcí příkaz rezidentně instalovaného programu **HC602** byl změněn z povelu &lt;Ctrl O&gt;&lt;L&gt; na &lt;Ctrl Alt&gt;&lt;O&gt;.
- při tisku více kopií je možnost zvolit režim "**Spojitě**" nebo "**Po stránkách**". Bližší informace najdete v nápovědě k příkazu **počet Kopií**.
- Každá klávesnice si "pamatuje" polohu přepínače **Z &gt; Y**. Přepínač se nastaví podle polohy, kterou měl naposledy při použití té které klávesnice. Nápověda k rozložení kláves je zobrazena vždy pro standardní tvar; t.j. bez prohození "Z" a "Y".
- Vedle programu HC602 (Orion) je zařazena další grafická utilita: program PCXTF602. Program je určen ke konverzi souborů obsahujících obrazové informace ve formátu PCX do formátu TIFF. Program se spouští z příkazové řádky operačního systému:

PCXTF602 *PCX_SOUBOR TIFF_SOUBOR* [/B|/C]

parametr /B znamená bílé pozadí (default), /C znamená černé pozadí.

- Editor umožňuje načíst soubory označené atributem Read Only. Tyto soubory jsou indikovány hvězdičkou, zobrazenou před názvem souboru. Soubor můžete **pouze prohlížet**, nemůžete jej však uložit ! Pokud jste však již v souboru provedli "nenahraditelné" úpravy a té hvězdičky jste si všimli až pozdě, poradíme vám malý trik: označte text jako blok a uložte jej do nového souboru.
- Pokud spustíte Preview klávesovým povelem &lt;Ctrl PrtScr&gt;, ukazuje aktuální stránku textu. Spustíte-li jej příkazem Preview z menu Tisk, ukazuje stránky zvolené příkazy Od stránky a Do stránky.
- Budete-li chtít Váš Text602 trvale používat ve slovenském režimu, zadejte příkaz:

COPY /B T602.EXE + SLOV.MNU T602.EXE

Reklamace

Oprávněné reklamace budou vyřízeny **výměnným způsobem**. Vyměňuje se zásadně **celý** komplet dodávky (bez manuálu). Při telefonických dotazech zásadně uvádějte verzi editoru (3.00), své sériové číslo (uvedené na obalu diskety), případně verzi instalačního programu. Převážná většina reklamací je způsobena nekompatibilitou disketových mechanik. Při prvním neúspěchu proto nejprve zkuste instalovat z jiné disketové mechaniky nebo na jiném zařízení. **Před prvním spuštěním instalačních disket je zajistěte proti přepisu a tedy i zavirování (zalepte nebo u 3.5" přesuňte šoupátko do "otevřené" polohy)**.

Při problémech s instalací (je hlášena hardwarová chyba apod.) je (podle našich zkušeností) možné usuzovat na nákazu počítače virem. Nepokračujte v další práci bez důkladného otestování všech diskových médií a paměti počítače !!!!!
