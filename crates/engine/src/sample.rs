//! Built-in sample documents and templates (original text, generated in code).

use wordcraft_doc::para::InlineObject;
use wordcraft_doc::props::{Align, Border, BorderStyle, Borders, CharProps, NumRef, Rgb, TabAlign, TabLeader, TabStop, TextColor};
use wordcraft_doc::{Block, Document, ListKind, Paragraph, PartKind, Table, para_block};

fn para(text: &str) -> Paragraph {
    Paragraph::with_text(text, CharProps::default())
}

fn styled(text: &str, style: &str) -> Paragraph {
    para(text).styled(style)
}

/// Append text with formatting to a paragraph.
fn add(p: &mut Paragraph, text: &str, props: CharProps) {
    let at = p.len();
    let _ = p.insert_text(at, text, &props);
}

fn bold() -> CharProps {
    CharProps { bold: Some(true), ..Default::default() }
}
fn italic() -> CharProps {
    CharProps { italic: Some(true), ..Default::default() }
}

/// The showcase handbook's text in one language.
struct Handbook {
    title: &'static str,
    subtitle: &'static str,
    intro: [&'static str; 5], // plain, bold, plain, italic, plain
    why_head: &'static str,
    why: &'static str,
    quote: &'static str,
    agree_head: &'static str,
    agree: [(&'static str, &'static str); 4], // bold lead, rest
    equipment_head: &'static str,
    equipment: &'static str,
    table: [[&'static str; 4]; 5],
    opening_head: &'static str,
    opening: [&'static str; 4],
    membership_head: &'static str,
    membership: [&'static str; 3], // plain, bold colour, plain
    questions: [&'static str; 3],  // plain, link, plain
    header: &'static str,
}

const EN: Handbook = Handbook {
    title: "The Open Studio Handbook",
    subtitle: "How a small community of makers shares space, tools and time",
    intro: [
        "Every Thursday evening the old print shop on Alder Street fills with people who make things. ",
        "Painters",
        " set up by the north windows, ",
        "letterpress",
        " regulars claim the back room, and somebody always brings bread. This handbook collects what we have learned about running a shared studio: the routines that keep it pleasant, the tools we pool, and the small agreements that let thirty strangers become a crew.",
    ],
    why_head: "Why share a studio?",
    why: "Rent is the obvious reason, but it is not the best one. A shared room puts your work next to work you would never have seen otherwise. A ceramicist explains glaze chemistry to a comic artist; a photographer borrows a weaver's loom for a still life. The cross-talk is the point.",
    quote: "A studio is a place where unfinished things are welcome.",
    agree_head: "What we agree on",
    agree: [
        ("Clean as you go. ", "Leave a table the way you would like to find it."),
        ("Label what is yours. ", "Unlabelled supplies on the commons shelf are fair game."),
        ("Ask before you borrow. ", "Even from the shelf, a quick note helps everyone keep track."),
        ("Quiet hours ", "run from 9 to 11 on weekday mornings."),
    ],
    equipment_head: "Shared equipment",
    equipment: "The table below lists what the membership owns together and who keeps each item in working order.",
    table: [
        ["Equipment", "Location", "Steward", "Booking"],
        ["Etching press", "Back room", "Mara O.", "Sign-up sheet"],
        ["Kiln (cone 10)", "Yard shed", "Theo P.", "48 h notice"],
        ["Large-format printer", "Office", "Ines R.", "Calendar"],
        ["Light table", "North windows", "Anyone", "First come"],
    ],
    opening_head: "Opening the studio",
    opening: [
        "Switch on the main breaker by the door.",
        "Open both skylights unless it is raining.",
        "Start the kettle. This step is not optional.",
        "Check the booking sheet for the kiln and press.",
    ],
    membership_head: "Membership",
    membership: [
        "Membership is ",
        "pay what you can",
        ", reviewed every season. New members shadow a regular for their first two visits, and everyone takes one cleaning shift a month. If you have never made anything before, you are exactly who we hoped would show up.",
    ],
    questions: ["Questions go to the front desk, or to the notice board by the sink. ", "Visit the community board", " for events."],
    header: "The Open Studio Handbook\t\tSpring edition",
};

const KA: Handbook = Handbook {
    title: "ღია სტუდიის სახელმძღვანელო",
    subtitle: "როგორ იყოფს შემოქმედთა პატარა საზოგადოება სივრცეს, ხელსაწყოებსა და დროს",
    intro: [
        "ყოველ ხუთშაბათს, საღამოს, მურყნის ქუჩაზე ძველი სტამბა ივსება ადამიანებით, რომლებიც რაღაცას ქმნიან. ",
        "მხატვრები",
        " ჩრდილოეთ ფანჯრებთან ეწყობიან, ",
        "ტიპოგრაფიული ბეჭდვის",
        " მოყვარულები უკანა ოთახს იკავებენ, პურს კი ყოველთვის ვიღაც მოიტანს. ეს სახელმძღვანელო აგროვებს ყველაფერს, რაც საერთო სტუდიის შესახებ ვისწავლეთ: ჩვევებს, რომლებიც გარემოს სასიამოვნოს ხდის, ხელსაწყოებს, რომლებსაც ერთად ვიყენებთ, და პატარა შეთანხმებებს, რომლებიც ოცდაათ უცნობს ერთ გუნდად აქცევს.",
    ],
    why_head: "რატომ საერთო სტუდია?",
    why: "ქირა აშკარა მიზეზია, მაგრამ არა საუკეთესო. საერთო ოთახში შენი ნამუშევარი ისეთი ნამუშევრების გვერდით ხვდება, რომლებსაც სხვაგვარად ვერასოდეს ნახავდი. კერამიკოსი კომიქსების მხატვარს ჭიქურის ქიმიას უხსნის; ფოტოგრაფი ნატურმორტისთვის მქსოველის დაზგას ისესხებს. მთავარი სწორედ ეს ურთიერთგაცვლაა.",
    quote: "სტუდია ის ადგილია, სადაც დაუსრულებელ ნამუშევრებს ყოველთვის ელიან.",
    agree_head: "რაზე ვთანხმდებით",
    agree: [
        ("დაალაგე მუშაობისას. ", "მაგიდა ისე დატოვე, როგორიც გინდა, რომ დაგხვდეს."),
        ("მონიშნე შენი ნივთები. ", "საერთო თაროზე უსახელოდ დატოვებული მასალებით ყველას შეუძლია ისარგებლოს."),
        ("იკითხე, სანამ ისესხებ. ", "თაროდან აღებისასაც კი პატარა ჩანაწერი ყველას ეხმარება, ნივთებს თვალი ადევნოს."),
        ("მშვიდი საათები ", "სამუშაო დღეებში დილის 9-დან 11-მდეა."),
    ],
    equipment_head: "საერთო აღჭურვილობა",
    equipment: "ქვემოთ მოცემულ ცხრილში ჩამოთვლილია, რას ფლობენ წევრები ერთად და ვინ ზრუნავს თითოეული ნივთის გამართულობაზე.",
    table: [
        ["აღჭურვილობა", "ადგილი", "პასუხისმგებელი", "დაჯავშნა"],
        ["საოფორტე წნეხი", "უკანა ოთახი", "ნინო ბ.", "ჩაწერის ფურცელი"],
        ["ღუმელი (კონუსი 10)", "ეზოს ფარდული", "გიორგი კ.", "48 სთ-ით ადრე"],
        ["დიდი ფორმატის პრინტერი", "ოფისი", "თამარ ლ.", "კალენდარი"],
        ["სინათლის მაგიდა", "ჩრდილოეთ ფანჯრები", "ნებისმიერი", "რიგის მიხედვით"],
    ],
    opening_head: "სტუდიის გახსნა",
    opening: [
        "ჩართე მთავარი ამომრთველი კართან.",
        "გააღე სახურავის ორივე ფანჯარა, თუ არ წვიმს.",
        "ჩართე ჩაიდანი. ეს ნაბიჯი სავალდებულოა.",
        "შეამოწმე ღუმელისა და წნეხის დაჯავშნის ფურცელი.",
    ],
    membership_head: "წევრობა",
    membership: [
        "წევრობის საფასური — ",
        "რამდენიც შეგიძლია",
        ", ყოველ სეზონზე გადაიხედება. ახალი წევრები პირველ ორ ვიზიტზე გამოცდილ წევრს ახლავან, და ყველა თვეში ერთ დასუფთავების ცვლას იღებს. თუ აქამდე არასოდეს არაფერი შეგიქმნია, სწორედ შენ გელოდით.",
    ],
    questions: ["კითხვებით მიმართე მისაღებს ან ნიჟარასთან დაკიდებულ საინფორმაციო დაფას. ", "ეწვიე საზოგადოების დაფას", " ღონისძიებების სანახავად."],
    header: "ღია სტუდიის სახელმძღვანელო\t\tსაგაზაფხულო გამოცემა",
};

/// The showcase document: a short studio handbook with headings, lists, a table, a quote,
/// a footer with page numbers. In English.
pub fn sample_document() -> Document {
    sample_document_in("en")
}

/// [`sample_document`] in an interface language (`ka` is Georgian); any other code gives English.
pub fn sample_document_in(lang: &str) -> Document {
    let t = if lang.get(..2).is_some_and(|l| l.eq_ignore_ascii_case("ka")) { &KA } else { &EN };
    let mut d = Document::new();
    d.core.title = t.title.into();
    d.core.creator = "WordCraft".into();
    let mut b: Vec<Block> = Vec::new();
    b.push(Block::Para(styled(t.title, "Title")));
    b.push(Block::Para(styled(t.subtitle, "Subtitle")));
    let mut intro = para(t.intro[0]);
    add(&mut intro, t.intro[1], bold());
    add(&mut intro, t.intro[2], CharProps::default());
    add(&mut intro, t.intro[3], italic());
    add(&mut intro, t.intro[4], CharProps::default());
    b.push(Block::Para(intro));
    b.push(Block::Para(styled(t.why_head, "Heading1")));
    b.push(Block::Para(para(t.why)));
    let mut quote = styled(t.quote, "Quote");
    quote.props.space_before = Some(12.0);
    b.push(Block::Para(quote));
    b.push(Block::Para(styled(t.agree_head, "Heading2")));
    let mut num = d.numbering.clone();
    let bullets = num.add_list(ListKind::Bullet);
    let steps = num.add_list(ListKind::Numbered);
    d.numbering = num;
    for (lead, rest) in t.agree {
        let mut p = Paragraph::new().styled("ListParagraph");
        add(&mut p, lead, bold());
        add(&mut p, rest, CharProps::default());
        p.props.numbering = Some(NumRef { num: bullets, level: 0 });
        b.push(Block::Para(p));
    }
    b.push(Block::Para(styled(t.equipment_head, "Heading2")));
    b.push(Block::Para(para(t.equipment)));
    let rows = &t.table;
    let mut tbl = Table::new(rows.len(), 4, 468.0);
    tbl.props.style = Some("GridTable4AccentBlue".into());
    for (r, row) in rows.iter().enumerate() {
        for (c, txt) in row.iter().enumerate() {
            if let Some(cell) = tbl.rows.get_mut(r).and_then(|x| x.cells.get_mut(c)) {
                cell.blocks = vec![para_block(para(txt))];
            }
        }
    }
    if let Some(r) = tbl.rows.first_mut() {
        r.props.header = true;
    }
    b.push(Block::Table(tbl));
    b.push(Block::Para(styled(t.opening_head, "Heading2")));
    for s in t.opening {
        let mut p = para(s).styled("ListParagraph");
        p.props.numbering = Some(NumRef { num: steps, level: 0 });
        b.push(Block::Para(p));
    }
    b.push(Block::Para(styled(t.membership_head, "Heading1")));
    let mut m = para(t.membership[0]);
    add(&mut m, t.membership[1], CharProps { bold: Some(true), color: Some(TextColor::Rgb(Rgb(0x15, 0x60, 0x82))), ..Default::default() });
    add(&mut m, t.membership[2], CharProps::default());
    b.push(Block::Para(m));
    let mut h = para(t.questions[0]);
    add(&mut h, t.questions[1], CharProps { style: Some("Hyperlink".into()), link: Some("https://getartcraft.com/".into()), ..Default::default() });
    add(&mut h, t.questions[2], CharProps::default());
    b.push(Block::Para(h));
    d.body = b.into_iter().map(std::sync::Arc::new).collect();
    // Footer: centred page number.
    let mut f = Paragraph::new().styled("Footer");
    f.props.align = Some(Align::Center);
    let _ = f.insert_object(
        0,
        InlineObject::Field { instr: "PAGE".into(), result: "1".into(), locked: false, dirty: false, code: false, form: String::new() },
        &CharProps::default(),
    );
    let fid = d.add_part(PartKind::Footer, vec![para_block(f)]);
    d.last_section.footers.default = Some(fid);
    let mut hd = styled(t.header, "Header");
    hd.props.borders = Some(Borders {
        bottom: Some(Border { style: BorderStyle::Single, width: 0.5, color: Some(Rgb(0xBF, 0xBF, 0xBF)), space: 4.0 }),
        ..Default::default()
    });
    let hid = d.add_part(PartKind::Header, vec![para_block(hd)]);
    d.last_section.headers.default = Some(hid);
    d
}

/// A block-style letter.
pub fn letter() -> Document {
    let mut d = Document::new();
    let lines: Vec<Paragraph> = vec![
        styled("Your Name", "Title"),
        para("123 Your Street · City, ST 00000 · you@example.com"),
        para(""),
        para(&crate::cmd::insert::format_date("MMMM d, yyyy")),
        para(""),
        para("Recipient Name"),
        para("Title, Company"),
        para("Street Address"),
        para(""),
        para("Dear Recipient,"),
        para(
            "Start with a sentence that says why you are writing. Keep paragraphs short and specific, and close with what you would like to happen next.",
        ),
        para("Thank you for your time."),
        para("Sincerely,"),
        para(""),
        para("Your Name"),
    ];
    d.body = lines.into_iter().map(para_block).collect();
    d
}

/// A one-page résumé.
pub fn resume() -> Document {
    let mut d = Document::new();
    let mut b: Vec<Block> =
        vec![Block::Para(styled("Alex Rivera", "Title")), Block::Para(para("Illustrator · alex@example.com · portfolio.example.com"))];
    for (head, items) in [
        ("Experience", vec![("Lead Illustrator, Harbor Books", "2021 – present"), ("Freelance Illustrator", "2016 – 2021")]),
        ("Education", vec![("BFA Illustration, Coastal College of Art", "2016")]),
        ("Skills", vec![("Ink, gouache, digital painting, book layout, lettering", "")]),
    ] {
        b.push(Block::Para(styled(head, "Heading1")));
        for (what, when) in items {
            let mut p = para(what);
            p.props.tabs = Some(vec![TabStop { pos: 468.0, align: TabAlign::Right, leader: TabLeader::None }]);
            if !when.is_empty() {
                add(&mut p, &format!("\t{when}"), italic());
            }
            b.push(Block::Para(p));
        }
    }
    d.body = b.into_iter().map(std::sync::Arc::new).collect();
    d
}

/// A report with a cover heading and sections.
pub fn report() -> Document {
    let mut d = Document::new();
    let b: Vec<Block> = vec![
        Block::Para(styled("Report Title", "Title")),
        Block::Para(styled("Subtitle or date", "Subtitle")),
        Block::Para(styled("Summary", "Heading1")),
        Block::Para(para("Summarise the findings in two or three sentences.")),
        Block::Para(styled("Background", "Heading1")),
        Block::Para(para("Describe the context and the question the report answers.")),
        Block::Para(styled("Findings", "Heading1")),
        Block::Para(styled("First finding", "Heading2")),
        Block::Para(para("Explain the evidence.")),
        Block::Para(styled("Recommendations", "Heading1")),
        Block::Para(para("List what should happen next.")),
    ];
    d.body = b.into_iter().map(std::sync::Arc::new).collect();
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_valid() {
        assert_eq!(sample_document().word_count(), 361);
        for d in [sample_document(), sample_document_in("ka"), letter(), resume(), report()] {
            assert!(d.word_count() > 5);
            let l = wordcraft_layout::layout(&d, &mut wordcraft_layout::LayoutCache::new(), &Default::default());
            assert!(!l.pages.is_empty());
        }
    }

    #[test]
    fn the_handbook_follows_the_interface_language() {
        assert_eq!(sample_document_in("ka").core.title, "ღია სტუდიის სახელმძღვანელო");
        assert_eq!(sample_document_in("ka-GE").core.title, "ღია სტუდიის სახელმძღვანელო");
        assert_eq!(sample_document_in("uk").core.title, "The Open Studio Handbook");
        assert_eq!(sample_document_in("").core.title, sample_document().core.title);
    }
}
