use std::{collections::HashSet, path::PathBuf};

use norad::{
    AffineTransform, Anchor, Color, GlifVersion, Glyph, Guideline, Identifier, Image, Line, Plist,
};
use quick_xml::{
    events::{attributes::Attributes, Event},
    Reader,
};

// use builder::OutlineBuilder;

// pub mod builder;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("failed to parse the XML structure")]
    Xml(#[source] quick_xml::Error),
    #[error("failed to parse the glif file: {0}")]
    Parse(ErrorKind),
}

#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    #[error("bad identifier")]
    BadIdentifier,
    #[error("found duplicate element")]
    DuplicateElement,
    #[error("duplicate identifier")]
    DuplicateIdentifier,
    #[error("invalid anchor element")]
    InvalidAnchor,
    #[error("an angle must be between 0 and 360°")]
    InvalidAngle,
    #[error("invalid codepoint '{0}': {1}")]
    InvalidCodepoint(String, Box<dyn std::error::Error>),
    #[error("invalid color attribute")]
    InvalidColor,
    #[error("invalid glyph element")]
    InvalidGlyph,
    #[error("invalid guideline element")]
    InvalidGuideline,
    #[error("invalid image element")]
    InvalidImage,
    #[error("invalid number '{0}': {1}")]
    InvalidInteger(String, std::num::ParseIntError),
    #[error("invalid number '{0}': {1}")]
    InvalidNumber(String, std::num::ParseFloatError),
    #[error("unvalid unicode element")]
    InvalidUnicode,
    #[error("the glyph lib must be a dictionary")]
    LibMustBeDictionary,
    #[error("failed to parse glyph lib")]
    ParsePlist(#[source] Box<dyn std::error::Error>),
    #[error("expected a single 'glyph' element in the glif file")]
    TrailingData,
    #[error("unexpected attribute")]
    UnexpectedAttribute,
    #[error("unexpected end of file")]
    UnexpectedEof,
    #[error("unsupported glif version")]
    UnsupportedGlifVersion,
    #[error("'glyph' must be the first element in a glif file")]
    WrongFirstElement,
}

pub fn parse_glif(xml: &[u8]) -> Result<Glyph, Error> {
    enum State {
        /// At the start of the glif buffer.
        Start,
        /// Inside the <glyph> element.
        Glyph(Glyph),
        // /// Inside the <outline> element.
        // Outline(Glyph, OutlineBuilder),
        // /// Inside the <contour> element.
        // Contour(Glyph, OutlineBuilder),
        /// Done with <glyph> and expecting the end of the file.
        Done(Glyph),
    }

    let mut reader = Reader::from_reader(xml);
    reader.trim_text(true);
    let mut state = State::Start;
    let mut buf = Vec::with_capacity(xml.len());
    let mut identifier_set: HashSet<Identifier> = HashSet::new();
    let mut seen_advance = false; // TODO: integrate seen_* into state above?
    let mut seen_lib = false;
    // let mut seen_outline = false;

    // TODO: deal with unexpected elements in v1
    loop {
        state = match (state, reader.read_event(&mut buf).map_err(Error::Xml)?) {
            (state, Event::Comment(_)) => state,
            (state, Event::Decl(_)) => state,

            // The first and only element must be a <glyph>.
            (State::Start, Event::Start(e)) if e.name() == b"glyph" => {
                let glyph = parse_glyph(&reader, e.attributes())?;
                State::Glyph(glyph)
            }
            (State::Start, Event::Empty(_) | Event::Start(_)) => {
                return Err(Error::Parse(ErrorKind::WrongFirstElement))
            }

            // Handle immediate child elements of <glyph>.
            (State::Glyph(mut glyph), Event::Empty(e)) if e.name() == b"unicode" => {
                let codepoint = parse_unicode(&reader, e.attributes())?;
                glyph.codepoints.push(codepoint);
                State::Glyph(glyph)
            }
            (State::Glyph(mut glyph), Event::Empty(e)) if e.name() == b"anchor" => {
                let anchor =
                    parse_anchor(&reader, e.attributes(), &mut identifier_set, &glyph.format)?;
                glyph.anchors.push(anchor);
                State::Glyph(glyph)
            }
            (State::Glyph(mut glyph), Event::Empty(e)) if e.name() == b"guideline" => {
                let guideline =
                    parse_guideline(&reader, e.attributes(), &mut identifier_set, &glyph.format)?;
                glyph.guidelines.push(guideline);
                State::Glyph(glyph)
            }
            (State::Glyph(mut glyph), Event::Empty(e)) if e.name() == b"advance" => {
                if seen_advance {
                    return Err(Error::Parse(ErrorKind::DuplicateElement));
                }
                seen_advance = true;
                let (height, width) = parse_advance(&reader, e.attributes())?;
                glyph.height = height;
                glyph.width = width;
                State::Glyph(glyph)
            }
            (State::Glyph(mut glyph), Event::Start(e)) if e.name() == b"note" => {
                if glyph.note.is_some() {
                    return Err(Error::Parse(ErrorKind::DuplicateElement));
                }
                let note = parse_note(&mut reader, &mut buf)?;
                glyph.note = Some(note);
                State::Glyph(glyph)
            }
            (State::Glyph(mut glyph), Event::Start(e)) if e.name() == b"lib" => {
                if seen_lib {
                    return Err(Error::Parse(ErrorKind::DuplicateElement));
                }
                seen_lib = true;
                let lib = parse_lib(&mut reader, &mut buf, xml)?;
                glyph.lib = lib;
                State::Glyph(glyph)
            }
            (State::Glyph(mut glyph), Event::Empty(e)) if e.name() == b"image" => {
                if glyph.image.is_some() {
                    return Err(Error::Parse(ErrorKind::DuplicateElement));
                }
                let image = parse_image(&reader, e.attributes())?;
                glyph.image = Some(image);
                State::Glyph(glyph)
            }

            // Finish up and expect the end of the file.
            (State::Glyph(glyph), Event::End(e)) if e.name() == b"glyph" => {
                // TODO: move object libs
                State::Done(glyph)
            }
            (State::Done(glyph), Event::Eof) => return Ok(glyph),
            (State::Done(_), _) => return Err(Error::Parse(ErrorKind::TrailingData)),

            // Anything else is an error.
            (_, Event::Eof) => return Err(Error::Parse(ErrorKind::UnexpectedEof)),
            (state, _) => state, // TODO: error out
        };
        buf.clear();
    }
}

fn parse_glyph(reader: &Reader<&[u8]>, attributes: Attributes) -> Result<Glyph, Error> {
    let mut name = String::new();
    let mut format: Option<GlifVersion> = None;
    let mut format_minor: u32 = 0;

    for attr in attributes {
        let attr = attr.map_err(Error::Xml)?;
        let value = attr.unescaped_value().map_err(Error::Xml)?;
        let value = reader.decode(&value).map_err(Error::Xml)?;
        match attr.key {
            b"name" => name.push_str(value),
            b"format" => {
                format = match value {
                    "1" => Some(GlifVersion::V1),
                    "2" => Some(GlifVersion::V2),
                    _ => return Err(Error::Parse(ErrorKind::UnsupportedGlifVersion)),
                }
            }
            b"formatMinor" => {
                format_minor = value
                    .parse()
                    .map_err(|e| Error::Parse(ErrorKind::InvalidInteger(value.into(), e)))?;
            }
            _ => return Err(Error::Parse(ErrorKind::UnexpectedAttribute)),
        }
    }

    if !name.is_empty() && format.is_some() {
        let mut glyph = Glyph::new_named(name);
        glyph.format = format.take().unwrap();
        // The formatMinor attribute is a UFO v3 thing, but it may not be
        // worth the hassle to be really pedantic about it.
        glyph.format_minor = format_minor;

        Ok(glyph)
    } else {
        Err(Error::Parse(ErrorKind::InvalidGlyph))
    }
}

fn parse_advance(reader: &Reader<&[u8]>, attributes: Attributes) -> Result<(f64, f64), Error> {
    let mut width: f64 = 0.0;
    let mut height: f64 = 0.0;

    for attr in attributes {
        let attr = attr.map_err(Error::Xml)?;
        let value = attr.unescaped_value().map_err(Error::Xml)?;
        let value = reader.decode(&value).map_err(Error::Xml)?;
        match attr.key {
            b"height" => height = parse_number(value)?,
            b"width" => width = parse_number(value)?,
            _ => return Err(Error::Parse(ErrorKind::UnexpectedAttribute)),
        }
    }

    Ok((height, width))
}

fn parse_unicode(reader: &Reader<&[u8]>, attributes: Attributes) -> Result<char, Error> {
    let mut codepoint = None;

    for attr in attributes {
        let attr = attr.map_err(Error::Xml)?;
        let value = attr.unescaped_value().map_err(Error::Xml)?;
        let value = reader.decode(&value).map_err(Error::Xml)?;
        match attr.key {
            b"hex" => codepoint = Some(parse_codepoint(value)?),
            _ => return Err(Error::Parse(ErrorKind::UnexpectedAttribute)),
        }
    }

    match codepoint {
        Some(chr) => Ok(chr),
        None => Err(Error::Parse(ErrorKind::InvalidUnicode)),
    }
}

fn parse_anchor(
    reader: &Reader<&[u8]>,
    attributes: Attributes,
    identifier_set: &mut HashSet<Identifier>,
    glif_format: &GlifVersion,
) -> Result<Anchor, Error> {
    let mut x: Option<f64> = None;
    let mut y: Option<f64> = None;
    let mut name: Option<String> = None;
    let mut color: Option<Color> = None;
    let mut identifier: Option<Identifier> = None;

    for attr in attributes {
        let attr = attr.map_err(Error::Xml)?;
        let value = attr.unescaped_value().map_err(Error::Xml)?;
        let value = reader.decode(&value).map_err(Error::Xml)?;
        match attr.key {
            b"x" => x = Some(parse_number(value)?),
            b"y" => y = Some(parse_number(value)?),
            b"name" => name = Some(value.to_string()),
            b"color" => color = Some(parse_color(value)?),
            b"identifier" => {
                identifier = Some(parse_identifier(value, identifier_set, glif_format)?);
            }
            _ => return Err(Error::Parse(ErrorKind::UnexpectedAttribute)),
        }
    }

    match (x, y) {
        (Some(x), Some(y)) => Ok(Anchor::new(x, y, name, color, identifier, None)),
        _ => Err(Error::Parse(ErrorKind::InvalidAnchor)),
    }
}

fn parse_guideline(
    reader: &Reader<&[u8]>,
    attributes: Attributes,
    identifier_set: &mut HashSet<Identifier>,
    glif_format: &GlifVersion,
) -> Result<Guideline, Error> {
    let mut x: Option<f64> = None;
    let mut y: Option<f64> = None;
    let mut angle: Option<f64> = None;
    let mut name: Option<String> = None;
    let mut color: Option<Color> = None;
    let mut identifier: Option<Identifier> = None;

    for attr in attributes {
        let attr = attr.map_err(Error::Xml)?;
        let value = attr.unescaped_value().map_err(Error::Xml)?;
        let value = reader.decode(&value).map_err(Error::Xml)?;
        match attr.key {
            b"x" => x = Some(parse_number(value)?),
            b"y" => y = Some(parse_number(value)?),
            b"angle" => {
                let angle_value = parse_number(value)?;
                if !(0.0..=360.0).contains(&angle_value) {
                    return Err(Error::Parse(ErrorKind::InvalidAngle));
                }
                angle = Some(angle_value);
            }
            b"name" => name = Some(value.to_string()),
            b"color" => color = Some(parse_color(value)?),
            b"identifier" => {
                identifier = Some(parse_identifier(value, identifier_set, glif_format)?);
            }
            _ => return Err(Error::Parse(ErrorKind::UnexpectedAttribute)),
        }
    }

    let line = match (x, y, angle) {
        (Some(x), None, None) => Line::Vertical(x),
        (None, Some(y), None) => Line::Horizontal(y),
        (Some(x), Some(y), Some(degrees)) => Line::Angle { x, y, degrees },
        _ => return Err(Error::Parse(ErrorKind::InvalidGuideline)),
    };

    Ok(Guideline::new(line, name, color, identifier, None))
}

fn parse_note(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) -> Result<String, Error> {
    reader.read_text(b"note", buf).map_err(Error::Xml)
}

fn parse_lib(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>, xml: &[u8]) -> Result<Plist, Error> {
    // The plist crate currently uses a different XML parsing library internally, so
    // we can't pass over control to it directly. Instead, pass it the precise slice
    // of the raw buffer to parse.
    let start = reader.buffer_position();
    reader.read_to_end(b"lib", buf).map_err(Error::Xml)?;
    let end = reader.buffer_position();
    let plist_slice = &xml[start..end];

    let dict = plist::Value::from_reader_xml(plist_slice)
        .map_err(|source| Error::Parse(ErrorKind::ParsePlist(source.into())))?
        .into_dictionary()
        .ok_or(Error::Parse(ErrorKind::LibMustBeDictionary))?;

    Ok(dict)
}

fn parse_image(reader: &Reader<&[u8]>, attributes: Attributes) -> Result<Image, Error> {
    let mut filename: Option<PathBuf> = None;
    let mut color: Option<Color> = None;
    let mut transform = AffineTransform::default();

    for attr in attributes {
        let attr = attr.map_err(Error::Xml)?;
        let value = attr.unescaped_value().map_err(Error::Xml)?;
        let value = reader.decode(&value).map_err(Error::Xml)?;
        match attr.key {
            b"xScale" => transform.x_scale = parse_number(value)?,
            b"xyScale" => transform.xy_scale = parse_number(value)?,
            b"yxScale" => transform.yx_scale = parse_number(value)?,
            b"yScale" => transform.y_scale = parse_number(value)?,
            b"xOffset" => transform.x_offset = parse_number(value)?,
            b"yOffset" => transform.y_offset = parse_number(value)?,
            b"color" => color = Some(parse_color(value)?),
            b"fileName" => filename = Some(PathBuf::from(value.to_string())),
            _ => return Err(Error::Parse(ErrorKind::UnexpectedAttribute)),
        }
    }

    match filename {
        Some(file_name) => Ok(Image {
            file_name,
            color,
            transform,
        }),
        None => Err(Error::Parse(ErrorKind::InvalidImage)),
    }
}

fn parse_codepoint(value: &str) -> Result<char, Error> {
    let i = u32::from_str_radix(value, 16)
        .map_err(|e| Error::Parse(ErrorKind::InvalidCodepoint(value.into(), e.into())))?;
    char::try_from(i).map_err(|e| Error::Parse(ErrorKind::InvalidCodepoint(value.into(), e.into())))
}

fn parse_number(value: &str) -> Result<f64, Error> {
    value
        .parse()
        .map_err(|e| Error::Parse(ErrorKind::InvalidNumber(value.into(), e)))
}

fn parse_color(value: &str) -> Result<Color, Error> {
    value
        .parse()
        .map_err(|_| Error::Parse(ErrorKind::InvalidColor))
}

fn parse_identifier(
    value: &str,
    identifier_set: &mut HashSet<Identifier>,
    glif_format: &GlifVersion,
) -> Result<Identifier, Error> {
    if *glif_format == GlifVersion::V1 {
        return Err(Error::Parse(ErrorKind::UnexpectedAttribute));
    }
    let id = Identifier::new(value).map_err(|_| Error::Parse(ErrorKind::BadIdentifier))?;
    if !identifier_set.insert(id.clone()) {
        return Err(Error::Parse(ErrorKind::DuplicateIdentifier));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parse_all() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <glyph name="period" format="2" formatMinor="123">
            <unicode hex="002E"/>
            <unicode hex="04D2"/>
            <advance height="123" width="268"/>
            <image fileName="period sketch.png" xScale="0.5" xyScale="0.5" yxScale="0.5" yScale="0.5" xOffset="0.5" yOffset="0.5" color="1,0,0,0.5"/>
            <outline>
                <contour identifier="vMlVuTQd4d">
                    <point x="237" y="152"/>
                    <point x="193" y="187"/>
                    <point x="134" y="187" type="curve" smooth="yes" identifier="KN3WZjorob"/>
                    <point x="74" y="187"/>
                    <point x="30" y="150"/>
                    <point name="median" x="30" y="88" type="curve" smooth="yes"/>
                    <point x="30" y="23"/>
                    <point x="74.123" y="-10.456"/>
                    <point x="134" y="-10" type="curve" smooth="yes"/>
                    <point x="193" y="-10"/>
                    <point x="237" y="25"/>
                    <point name="end" x="237" y="88" type="curve" smooth="yes" identifier="h0ablXAzTg"/>
                </contour>
                <component base="A" identifier="c1"/>
                <component base="A" xScale="2" xyScale="2" yxScale="2" yScale="2" xOffset="2" yOffset="2" identifier="c2"/>
                <component base="A" xScale="1.234" xyScale="1.234" yxScale="1.234" yScale="1.234" xOffset="1.234" yOffset="1.234" identifier="c3"/>
            </outline>
            <anchor name="top" x="74" y="197" color="0,0,0,0" identifier="a1"/>
            <anchor name="elsewhere" x="1.234" y="5.678" color="1,0,0,1" identifier="a2"/>
            <guideline name="overshoot" y="-12" color="1,0,0,1" identifier="g1"/>
            <guideline name="baseline" x="0.1" color="0,1,0,1" identifier="g2"/>
            <guideline name="diagonals" x="100.2" y="200.4" angle="360" color="0,0,1,1" identifier="g3"/>
            <lib>
                <dict>
                    <key>com.letterror.somestuff</key>
                    <string>arbitrary custom data!</string>
                    <key>public.markColor</key>
                    <string>1,0,0,0.5</string>
                    <key>public.objectLibs</key>
                    <dict>
                        <key>KN3WZjorob</key>
                        <dict>
                            <key>com.foundry.pointColor</key>
                            <string>0,1,0,0.5</string>
                        </dict>
                        <key>a1</key>
                        <dict>
                            <key>asdf</key>
                            <integer>0</integer>
                        </dict>
                        <key>a2</key>
                        <dict>
                            <key>asdf</key>
                            <integer>1</integer>
                        </dict>
                        <key>c1</key>
                        <dict>
                            <key>asdf</key>
                            <integer>0</integer>
                        </dict>
                        <key>c2</key>
                        <dict>
                            <key>asdf</key>
                            <integer>1</integer>
                        </dict>
                        <key>c3</key>
                        <dict>
                            <key>asdf</key>
                            <integer>2</integer>
                        </dict>
                        <key>g1</key>
                        <dict>
                            <key>asdf</key>
                            <integer>0</integer>
                        </dict>
                        <key>g2</key>
                        <dict>
                            <key>asdf</key>
                            <integer>1</integer>
                        </dict>
                        <key>g3</key>
                        <dict>
                            <key>asdf</key>
                            <integer>2</integer>
                        </dict>
                        <key>h0ablXAzTg</key>
                        <dict>
                            <key>com.foundry.pointColor</key>
                            <string>1,0,0,0.5</string>
                        </dict>
                        <key>vMlVuTQd4d</key>
                        <dict>
                            <key>com.foundry.contourColor</key>
                            <string>1,0,0,0.5</string>
                        </dict>
                    </dict>
                    <key>public.postscript.hints</key>
                    <dict>
                        <key>formatVersion</key>
                        <string>1</string>
                        <key>hintSetList</key>
                        <array>
                            <dict>
                                <key>pointTag</key>
                                <string>hintSet0000</string>
                                <key>stems</key>
                                <array>
                                    <string>hstem -10 197</string>
                                    <string>vstem 30 207</string>
                                </array>
                            </dict>
                            <dict>
                                <key>pointTag</key>
                                <string>hintSet0004</string>
                                <key>stems</key>
                                <array>
                                    <string>hstem 11 -21</string>
                                    <string>vstem 30 207</string>
                                </array>
                            </dict>
                        </array>
                        <key>id</key>
                        <string>w268c237,88 237,152 193,187c134,187 74,187 30,150c30,88 30,23 74,-10c134,-10 193,-10 237,25</string>
                    </dict>
                </dict>
            </lib><note>I äm a note.</note></glyph>
        "#;

        let glyph = parse_glif(xml.as_bytes()).unwrap();

        assert_eq!(glyph.name, "period".into());
        assert_eq!(glyph.format, GlifVersion::V2);
        assert_eq!(glyph.format_minor, 123);

        assert_eq!(glyph.height, 123.0);
        assert_eq!(glyph.width, 268.0);

        assert_eq!(glyph.codepoints, vec!['\u{002E}', '\u{04D2}']);

        assert_eq!(
            glyph.anchors,
            vec![
                Anchor::new(
                    74.0,
                    197.0,
                    Some("top".into()),
                    Some(Color {
                        red: 0.0,
                        green: 0.0,
                        blue: 0.0,
                        alpha: 0.0
                    }),
                    Some(Identifier::new("a1").unwrap()),
                    None
                ),
                Anchor::new(
                    1.234,
                    5.678,
                    Some("elsewhere".into()),
                    Some(Color {
                        red: 1.0,
                        green: 0.0,
                        blue: 0.0,
                        alpha: 1.0
                    }),
                    Some(Identifier::new("a2").unwrap()),
                    None
                )
            ]
        );

        assert_eq!(
            glyph.guidelines,
            vec![
                Guideline::new(
                    Line::Horizontal(-12.0),
                    Some("overshoot".into()),
                    Some(Color {
                        red: 1.0,
                        green: 0.0,
                        blue: 0.0,
                        alpha: 1.0
                    }),
                    Some(Identifier::new("g1").unwrap()),
                    None
                ),
                Guideline::new(
                    Line::Vertical(0.1),
                    Some("baseline".into()),
                    Some(Color {
                        red: 0.0,
                        green: 1.0,
                        blue: 0.0,
                        alpha: 1.0
                    }),
                    Some(Identifier::new("g2").unwrap()),
                    None
                ),
                Guideline::new(
                    Line::Angle {
                        x: 100.2,
                        y: 200.4,
                        degrees: 360.0
                    },
                    Some("diagonals".into()),
                    Some(Color {
                        red: 0.0,
                        green: 0.0,
                        blue: 1.0,
                        alpha: 1.0
                    }),
                    Some(Identifier::new("g3").unwrap()),
                    None
                )
            ]
        );

        assert_eq!(
            glyph.image.unwrap(),
            Image {
                file_name: PathBuf::from("period sketch.png"),
                color: Some(Color {
                    red: 1.0,
                    green: 0.0,
                    blue: 0.0,
                    alpha: 0.5
                }),
                transform: AffineTransform {
                    x_scale: 0.5,
                    xy_scale: 0.5,
                    yx_scale: 0.5,
                    y_scale: 0.5,
                    x_offset: 0.5,
                    y_offset: 0.5
                }
            }
        );

        let mut lib_keys: Vec<&str> = glyph.lib.keys().map(|s| s.as_str()).collect();
        lib_keys.sort_unstable();
        assert_eq!(
            lib_keys,
            vec![
                "com.letterror.somestuff",
                "public.markColor",
                "public.objectLibs",
                "public.postscript.hints",
            ]
        );

        assert_eq!(glyph.note, Some("I äm a note.".into()));
    }

    #[test]
    #[should_panic(expected = "WrongFirstElement")]
    fn wrong_first_element() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <unicode hex="002E"/>
        "#;

        let _ = parse_glif(xml.as_bytes()).unwrap();
    }

    #[test]
    #[should_panic(expected = "DuplicateElement")]
    fn duplicate_note() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <glyph name="period" format="2" formatMinor="123">
            <note>I äm a note.</note>
            <note>I äm a note.</note>
        </glyph>
        "#;

        let _ = parse_glif(xml.as_bytes()).unwrap();
    }

    #[test]
    #[should_panic(expected = "DuplicateElement")]
    fn duplicate_advance() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <glyph name="period" format="2" formatMinor="123">
            <advance height="123" width="268"/>
            <advance height="123" width="268"/>
        </glyph>
        "#;

        let _ = parse_glif(xml.as_bytes()).unwrap();
    }

    #[test]
    #[should_panic(expected = "DuplicateElement")]
    fn duplicate_lib() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <glyph name="period" format="2" formatMinor="123">
            <lib>
                <dict>
                        <key>formatVersion</key>
                        <string>1</string>
                </dict>
            </lib>
            <lib>
                <dict>
                        <key>formatVersion</key>
                        <string>1</string>
                </dict>
            </lib>
        </glyph>
        "#;

        let _ = parse_glif(xml.as_bytes()).unwrap();
    }

    #[test]
    #[should_panic(expected = "TrailingData")]
    fn trailing_data() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <glyph name="period" format="2" formatMinor="123">
        </glyph>
        <unicode hex="002E"/>
        "#;

        let _ = parse_glif(xml.as_bytes()).unwrap();
    }
}
