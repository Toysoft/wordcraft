# Arabic interface

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** trivial (screenshot pass: crop sides, Border, units; previously minor: status, what is done and known gaps; previously minor: Arabic catalog complete) · **Target:** WordCraft 0.7.x

Choose **File › Options › Interface language › العربية**, or follow an Arabic system display
language (`ar`, `ar-SA`, `ar_EG.UTF-8`, any `ar-*`). `ui.language` with `{"value":"ar"}` selects it
explicitly. Document text, command ids, CLI/MCP/control responses and file paths do not change.

`crates/ui-egui/src/i18n/ar.tsv` is written in Modern Standard Arabic from the meaning of the English
labels (clean-room; no product localization consulted). It covers the whole reference key set (every
key of the Polish catalog, 2,463 entries): ribbon, menus, dialogs, panes, Backstage, built-in style
names, the equation gallery, watermarks and preview tiles; `arabic_covers_every_key_of_the_polish_catalog`
keeps it complete. A string added later shows in English until it is translated. The catalog is
written in logical order; the parser stores it in draw order (`bidi::visual`).

## Status: partial (native review pending)

**Done**

- Catalog: 2,463 entries, every key of the Polish catalog; `arabic_covers_every_key_of_the_polish_catalog` keeps it complete.
- Mirrored interface: title bar, status bar, ribbon, menus, panes, dialogs, Backstage, tooltips and icons ([`ui-parity.md`](ui-parity.md#mirrored-interface-arabic)). Hebrew needs only a catalog plus `rtl: true` on its `LangInfo`.
- Interface text is reordered by `bidi.rs` (our own UAX #9 pass, because epaint draws neutrals separately and does not mirror brackets); counts, locations and tooltips are composed in logical order, then converted once (`i18n::fmt`, `fmt_translated`, `suffixed`).
- User and document text in the chrome (navigation headings, find results, comments, recent files, window caption, style names, bookmarks, the Organizer, preview tiles, the default title "مستند1") is drawn in its own order (`i18n::user_text`, `t_logical`), and file names use the interface direction.
- Fonts: an Arabic face after Inter in every interface weight; craft-fonts lookup plus installed Segoe UI, Tahoma, SF Arabic, Geeza Pro or Noto Sans Arabic. The craft-fonts UI face is an owner action.
- `ui.language` with `{"value":"ar"}`, File > Options and any `ar-*` system locale select it.

**Known gaps** (ranked in [gaps.md](gaps.md#arabic-interface-follow-ups))

- No native-speaker review; wording follows the meaning of the English labels.
- Text fields (`TextEdit`) edit in logical order: caret, selection and Home/End are not bidi-aware.
- Scroll bars of egui `ScrollArea` stay at the right; combo-box internals are not mirrored.
- No Arabic spelling or grammar proofing; no kashida.
- The shared keys "Display" and "Bar" have one translation each but two meanings in the interface.
- Command search takes logical queries but compares against draw-order labels, so Arabic queries match poorly.
- The page-range and author parsers accept `,` and `;` but not the Arabic comma and semicolon.
- The mailings envelope label "#10   4.13 x 9.5 in" is not wrapped for translation and shows in English (the crop sides and Picture Format Border were added after the screenshot pass; "Table Grid" was already translated).

## Conventions

- Digits stay Western (0-9). Arabic punctuation (، ؟ ؛) in running text; trailing `…` kept.
- Counts use "noun: {n}" wording (`الكلمات: {words}`): Arabic has six plural forms and the catalog has none.
- Commands are verbal nouns (`حفظ`, `نسخ`, `لصق`); placeholders never take a prefix or ending.
- Product and technical names stay Latin (Zotero, Discord, SmartArt, WordArt, PDF, PNG, Unicode).
- Left/Right in commands name the physical side (`يسار`, `يمين`).
- Syntax hints keep ASCII separators (`صفحات مثل 1-3, 5`, `العائلة, الاسم; العائلة, الاسم`): the page-range and author parsers read `,` and `;`, not `،` or `؛`.
- Option labels are verbal nouns (`استخدام «a» قبل صوت ساكن`); tooltips describe in the present tense (`يعرض المستندات…`).

## Terminology

| Source | Arabic |
|---|---|
| File / Home / Insert / Draw / Design / Layout | ملف / الصفحة الرئيسية / إدراج / رسم / تصميم / تخطيط |
| References / Mailings / Review / View / Help | المراجع / المراسلات / مراجعة / عرض / مساعدة |
| Save / Save As / Open / New / Close / Print | حفظ / حفظ باسم / فتح / جديد / إغلاق / طباعة |
| Undo / Redo / Repeat | تراجع / إعادة / تكرار |
| Cut / Copy / Paste / Clipboard | قص / نسخ / لصق / الحافظة |
| Bold / Italic / Underline / Strikethrough | غامق / مائل / تسطير / يتوسطه خط |
| Subscript / Superscript | منخفض / مرتفع |
| Font / Font Size / Font Color | الخط / حجم الخط / لون الخط |
| Paragraph / Style / Styles | الفقرة / النمط / الأنماط |
| Heading / Title (style) | عنوان / العنوان الرئيسي |
| Header / Footer | الرأس / التذييل |
| Footnote / Endnote | حاشية سفلية / حاشية ختامية |
| Table / Row / Column / Cell | جدول / صف / عمود / خلية |
| Picture / Shape / Chart / Text Box | صورة / شكل / مخطط / مربع نص |
| Margins / Orientation / Page Setup | الهوامش / الاتجاه / إعداد الصفحة |
| Line Spacing / Indent | تباعد الأسطر / مسافة بادئة |
| Bullets / Numbering / Multilevel List | تعداد نقطي / ترقيم / قائمة متعددة المستويات |
| Find / Replace / Select | بحث / استبدال / تحديد |
| Track Changes / Comments | تتبع التغييرات / التعليقات |
| Accept / Reject | قبول / رفض |
| Spelling & Grammar / Thesaurus | الإملاء والنحو / مرادفات |
| Word Count / Language | عدد الكلمات / اللغة |
| Table of Contents / Index / Bibliography | جدول المحتويات / الفهرس / قائمة المراجع |
| Citation / Cross-reference / Bookmark | استشهاد / إسناد مرجعي / إشارة مرجعية |
| Change to Display / Inline (equation) | تحويل إلى معادلة مستقلة / ضمن السطر |
| Research (kept distinct from Search) | بحث معرفي |
| Watermark / Page Color / Page Borders | علامة مائية / لون الصفحة / حدود الصفحة |
| Zoom / Ruler / Gridlines | تكبير/تصغير / المسطرة / خطوط الشبكة |
| Print Layout / Web Layout / Read Mode | تخطيط الطباعة / تخطيط ويب / وضع القراءة |
| Options / Properties / Info | خيارات / الخصائص / معلومات |
| OK / Cancel / Apply / Yes / No | موافق / إلغاء / تطبيق / نعم / لا |
| Field / Field code / Merge field | حقل / رمز الحقل / حقل دمج |
| Recipient / Recipient list / Mail Merge | مستلم / قائمة المستلمين / دمج المراسلات |
| Label (mailing) / Envelope | ملصق / مظروف |
| Password / Built-in style / Linked style | كلمة المرور / نمط مضمّن / نمط مرتبط |
| Tab stop / Leader | نقطة جدولة / الخط الموجّه |
| Caret / Overtype | مؤشر الكتابة / استبدال أثناء الكتابة |
| Balloons / Revisions (tracked) | الفقاعات / التغييرات (المتعقَّبة) |
| Hyphenate / Hyphenation zone | تقسيم الكلمات / منطقة التقسيم |
| Wildcards / Regular expressions / Match case / Whole words | أحرف البدل / التعابير النمطية / مطابقة حالة الأحرف / الكلمات الكاملة فقط |
| pt (unit) / Inches / Centimeters | نقطة / بوصات / سنتيمترات |
| Interface language / Interface size / Animation | لغة الواجهة / حجم الواجهة / الحركة |
| Caption / Label (caption) / Source / Entry (index, TOC) | تسمية توضيحية / تسمية / مصدر / إدخال |
| Series / Category (chart) / Accent (theme colour) | سلسلة / فئة / لون مميز |
| Restrict Editing / Protection / Tracked change | تقييد التحرير / الحماية / تغيير متعقَّب |
| AutoCorrect / AutoFormat As You Type | التصحيح التلقائي / التنسيق التلقائي أثناء الكتابة |
| Shortcut / Ribbon tab / Group | اختصار / علامة تبويب الشريط / مجموعة |
| Commit / PR (contributors page) | إيداع / طلب دمج |
| Built-in styles: Heading {n} / TOC {n} / Index {n} / Subtitle / No Spacing | عنوان {n} / جدول المحتويات {n} / فهرس {n} / عنوان فرعي / بلا تباعد |
| Emphasis / Strong / Subtle / Intense / Quote | تأكيد / قوي / خفيف / مكثف / اقتباس |
| Grid Table / List Table / Plain Table | جدول شبكي / جدول قائمة / جدول عادي |
| Equation: Fraction / Script / Radical / Integral / Large Operator | كسر / نص مرتفع/منخفض / جذر / تكامل / عامل كبير |
| Equation: Accent(s) / Logical AND / OR / Ceiling / Floor | علامة فوقية (العلامات الفوقية) / العطف المنطقي / الفصل المنطقي / دالة السقف / دالة الأرضية |
| Section Break / Lasso / Ink stroke | فاصل قسم / تحديد حر / ضربة القلم |
| Rich Text Format / Typography / Border Painter / Hue | تنسيق النص الغني / فن الخط / فرشاة الحدود / درجة اللون |
| SmartArt layouts (Basic Venn…) | مخطط فن أساسي … |
| Bracket(s) / Parentheses / Braces / Matrix / Function / Limit | أقواس مربعة / أقواس / أقواس معقوصة / مصفوفة / دالة / النهاية |
| Linear / Professional (equation form) | خطي / احترافي |
| Watermarks DRAFT / CONFIDENTIAL / URGENT / SAMPLE | مسودة / سري / عاجل / نموذج |
| Paper sizes (Letter, Legal, Executive, Tabloid, B4 (JIS)…) | kept in Latin (standard names); Envelope → مظروف |
| Glow / Soft Edges / Reflection / Shadow | توهج / حواف ناعمة / انعكاس / ظل |
| Connector (straight, elbow, curved) | موصل (مستقيم، مرفقي، منحنٍ) |
| Theme Colors / Theme Fonts / Standard Colors | ألوان السمة / خطوط السمة / الألوان القياسية |
| Wrap: In Line with Text / Behind Text / In Front of Text | مع النص في السطر / خلف النص / أمام النص |
| All Markup / No Markup | كل العلامات / بلا علامات |
| UPPERCASE / lowercase / tOGGLE cASE | أحرف كبيرة / أحرف صغيرة / تبديل حالة الأحرف (Arabic has no case: the label names the action) |
| Language names | Arabic adjective with the article: الإنجليزية (الولايات المتحدة)، الفرنسية (فرنسا)، العربية |

## Revision history

- 2026-10-14: first version with the `ar` catalog (L10N-AR.5).
- 2026-10-14: wording fixes (Title, Citation, outlines, equation display) (L10N-AR.5).
- 2026-10-15: dialogs part 1 (font, paragraph, layout, insert, lists, options, files) (L10N-AR.15a).
- 2026-10-15: dialogs part 2, task panes, Backstage, credits (L10N-AR.15b).
- 2026-10-15: built-in style names, equation gallery, watermarks, paper sizes, preview tiles (L10N-AR.15c).
- 2026-10-15: remaining ribbon labels, menus, context menus, SmartArt and chart galleries; catalog complete (L10N-AR.15d).
- 2026-10-15: review fixes: ASCII separators in syntax hints, maths accents, section break, lasso, logic and rounding functions, RTF, size boxes (L10N-AR.15).
- 2026-10-15: status, what is done and known gaps; mirrored interface and user-text work described (L10N-AR.16).
- 2026-10-15: side-by-side screenshot pass: crop sides (يسار، أعلى، يمين، أسفل) and Border (الحد) added; size boxes use نقطة (L10N-AR.16).
