use notebender::musicxml::*;

const SIMPLE: &str = include_str!("fixtures/simple.musicxml");

#[test]
fn key_parse_table() {
    let cases = [
        ("C", 0, Mode::Major),
        ("G", 1, Mode::Major),
        ("G major", 1, Mode::Major),
        ("F", -1, Mode::Major),
        ("Bb", -2, Mode::Major),
        ("B-flat major", -2, Mode::Major),
        ("B♭ major", -2, Mode::Major),
        ("F# minor", 3, Mode::Minor),
        ("F sharp minor", 3, Mode::Minor),
        ("f#m", 3, Mode::Minor),
        ("a", 0, Mode::Minor),
        ("A min", 0, Mode::Minor),
        ("bb", -5, Mode::Minor),
        ("Ebm", -6, Mode::Minor),
        ("A-", -4, Mode::Major),
        ("C#", 7, Mode::Major),
        ("Cb", -7, Mode::Major),
    ];
    for (s, f, m) in cases {
        assert_eq!(Key::parse(s).unwrap(), Key { fifths: f, mode: m }, "{s}");
    }
    assert!(Key::parse("H").is_err());
    assert!(Key::parse("D dorian").is_err());
    assert!(Key::parse("G# major").is_err());
    assert_eq!(Key::parse("Bb").unwrap().name(), "Bb major");
    assert_eq!(Key::parse("F# minor").unwrap().name(), "F# minor");
    assert_eq!(Key::parse("Bb").unwrap().tonic_step_alter(), ('B', -1));
}

#[test]
fn interval_selection() {
    let iv = |s, t| Interval { semitones: s, steps: t };
    assert_eq!(Interval::from_semitones(1, None), iv(1, 1));
    let g = Key::parse("G").unwrap();
    assert_eq!(Interval::from_semitones(1, Some(&g)), iv(1, 1));
    let b = Key {
        fifths: 5,
        mode: Mode::Major,
    };
    assert_eq!(Interval::from_semitones(1, Some(&b)), iv(1, 1));
    assert_eq!(Interval::from_semitones(-2, None), iv(-2, -1));
    assert_eq!(Interval::from_semitones(7, None), iv(7, 4));
    let c = Key::parse("C").unwrap();
    assert_eq!(Interval::between_keys(&g, &c), iv(5, 3));
    let fs = Key::parse("F#").unwrap();
    assert_eq!(Interval::between_keys(&c, &fs), iv(6, 3));
    assert_eq!(iv(2, 1).fifths_offset(), 2);
    assert_eq!(iv(1, 1).fifths_offset(), -5);
    assert_eq!(iv(7, 4).fifths_offset(), 1);
    assert_eq!(iv(3, 2).fifths_offset(), -3);
    assert_eq!(iv(2, 1).describe(), "up a major second (+2 semitones)");
    assert_eq!(iv(-3, -2).describe(), "down a minor third (-3 semitones)");
    assert_eq!(iv(0, 0).describe(), "unchanged");
    assert_eq!(iv(1, 0).describe(), "up an augmented unison (+1 semitones)");
    assert_eq!(iv(2, 1).inverted(), iv(-2, -1));
}

#[test]
fn transpose_up_major_second() {
    assert_eq!(
        detect_key(SIMPLE),
        Some(Key {
            fifths: 0,
            mode: Mode::Major
        })
    );
    let out = transpose(SIMPLE, &Interval { semitones: 2, steps: 1 }).unwrap();
    assert!(
        out.contains("<step>C</step>\n          <alter>1</alter>\n          <octave>5</octave>"),
        "{out}"
    );
    assert!(out.contains("<pitch><step>F</step><alter>1</alter><octave>4</octave></pitch>"));
    assert!(out.contains("<fifths>2</fifths>"));
    assert!(out.contains("<root-step>A</root-step>"));
    assert!(out.contains("<bass-step>C</bass-step><bass-alter>1</bass-alter>"));
    // B-flat4 + M2 = C5 natural
    assert!(out.contains("<step>C</step><alter>0</alter><octave>5</octave></pitch><duration>1</duration><accidental>natural</accidental>"));
    assert!(out.contains("<!-- keep me exactly -->"));
    assert!(out.contains("<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">"));
    assert!(out.contains("<note><rest/><duration>1</duration></note>"));
    // Zero interval is the identity, byte for byte.
    assert_eq!(transpose(SIMPLE, &Interval { semitones: 0, steps: 0 }).unwrap(), SIMPLE);
    validate(&out).unwrap();
}

#[test]
fn transpose_down_minor_second() {
    let out = transpose(
        SIMPLE,
        &Interval {
            semitones: -1,
            steps: -1,
        },
    )
    .unwrap();
    assert!(out.contains("<fifths>5</fifths>"));
    // C4 -> B3
    assert!(
        out.contains("<pitch><step>B</step><octave>3</octave></pitch><duration>4</duration>"),
        "{out}"
    );
    assert_eq!(detect_key(&out).unwrap().fifths, 5);
}

#[test]
fn transpose_key_overflow_errors() {
    let iv = Interval { semitones: 1, steps: 0 }; // augmented unison: +7 fifths
    let g = SIMPLE.replace("<fifths>0</fifths>", "<fifths>3</fifths>");
    assert!(transpose(&g, &iv).is_err());
    assert!(transpose("<a>", &iv).is_err());
}

#[test]
fn validate_and_summarize() {
    validate(SIMPLE).unwrap();
    let s = summarize(SIMPLE).unwrap();
    assert_eq!(s.title.as_deref(), Some("Test Piece"));
    assert_eq!(s.composer.as_deref(), Some("J. Doe"));
    assert_eq!(s.measures, 2);
    assert_eq!(s.note_count, 4);
    assert_eq!(s.time.as_deref(), Some("4/4"));
    assert_eq!(s.parts[0].clefs, vec!["G2"]);
    assert!(!s.has_tablature);
    assert_eq!(
        s.describe(),
        "1 part (Piano: 1 staff, G2 clefs; 2 measures), key C major, 4/4, 4 notes"
    );

    let timewise = r#"<score-timewise><part-list/></score-timewise>"#;
    assert!(matches!(validate(timewise), Err(ValidationError::WrongRoot(_))));
    assert!(matches!(validate("<a"), Err(ValidationError::NotXml(_))));
    let nolist = r#"<score-partwise><part id="P1"><measure number="1"/></part></score-partwise>"#;
    assert!(matches!(validate(nolist), Err(ValidationError::NoPartList)));
    let bad = SIMPLE.replace("<octave>4</octave>\n        </pitch>", "</pitch>");
    assert!(matches!(validate(&bad), Err(ValidationError::BadPitch { .. })));
    let mism = SIMPLE.replace("<part id=\"P1\">", "<part id=\"P9\">");
    assert!(matches!(validate(&mism), Err(ValidationError::PartListMismatch(_))));
}

fn page(part_id: &str, attrs: &str, nums: &[&str]) -> String {
    let ms: String = nums
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let a = if i == 0 { attrs } else { "" };
            format!("<measure number=\"{n}\">{a}<note><pitch><step>C</step><octave>4</octave></pitch></note></measure>")
        })
        .collect();
    format!(
        "<score-partwise><part-list><score-part id=\"P1\"><part-name>A</part-name></score-part></part-list><part id=\"{part_id}\">{ms}</part></score-partwise>"
    )
}

#[test]
fn merge_two_pages() {
    let attrs = "<attributes><divisions>1</divisions><key><fifths>1</fifths></key><time><beats>4</beats><beat-type>4</beat-type></time></attributes>";
    let attrs2 = "<attributes><divisions>1</divisions><key><fifths>1</fifths></key><time><beats>3</beats><beat-type>4</beat-type></time></attributes>";
    let p1 = page("P1", attrs, &["1", "2"]);
    let p2 = page("P1", attrs2, &["1", "2"]);
    let (out, w) = merge_pages(&[p1, p2]).unwrap();
    assert!(w.is_empty(), "{w:?}");
    for n in 1..=4 {
        assert!(out.contains(&format!("<measure number=\"{n}\"")), "{out}");
    }
    assert_eq!(out.matches("<divisions>").count(), 1);
    assert_eq!(out.matches("<fifths>").count(), 1);
    assert_eq!(out.matches("<time>").count(), 2); // differing time is kept
    assert_eq!(summarize(&out).unwrap().measures, 4);

    // Fully redundant attributes element vanishes.
    let p3 = page("P1", attrs, &["1"]);
    let (out, _) = merge_pages(&[page("P1", attrs, &["1"]), p3]).unwrap();
    assert_eq!(out.matches("<attributes>").count(), 1);
}

#[test]
fn merge_part_mismatch_warns() {
    let a = page("P1", "", &["1"]);
    let b = page("PX", "", &["1"]);
    let (out, w) = merge_pages(&[a.clone(), b]).unwrap();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("by position"));
    assert_eq!(out.matches("<measure ").count(), 2);

    let two = a
        .replace("</part-list>", "<score-part id=\"P2\"/></part-list>")
        .replace(
            "</part></score-partwise>",
            "</part><part id=\"P2\"><measure number=\"1\"/></part></score-partwise>",
        );
    let (_, w) = merge_pages(&[two, a]).unwrap();
    assert!(w.iter().any(|m| m.contains("missing")), "{w:?}");
}
