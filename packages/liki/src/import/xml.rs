//! Plain appfilter.xml through libexpat, the parser Python's ElementTree used (Python is built with the system's libexpat),
//! set up as ElementTree.fromstring sets it up: namespaces on ("uri}name" becomes "{uri}name"), internal entities expanded,
//! an undefined entity that expat hands on (a document with an external DTD) an error, as ElementTree made it. So a file
//! parses, or fails, as it did for the script. libexpat is opened at run time, only for a pack with a plain appfilter.xml.
//!
//! Not 1:1: ElementTree also read XML declared in an 8-bit encoding expat does not know itself (windows-1252, koi8-r ...)
//! through Python's codecs; here such a file does not parse (UTF-8, UTF-16, ISO-8859-1 and US-ASCII do).

use super::axml::{Element, Unparsable};
use std::ffi::{c_char, c_int, c_void, CStr};

type XmlParser = *mut c_void;
type StartHandler = unsafe extern "C" fn(*mut c_void, *const c_char, *mut *const c_char);
type EndHandler = unsafe extern "C" fn(*mut c_void, *const c_char);
type DataHandler = unsafe extern "C" fn(*mut c_void, *const c_char, c_int);

struct Expat {
    create_ns: unsafe extern "C" fn(*const c_char, c_char) -> XmlParser,
    set_user_data: unsafe extern "C" fn(XmlParser, *mut c_void),
    set_element_handler: unsafe extern "C" fn(XmlParser, StartHandler, EndHandler),
    set_character_data_handler: unsafe extern "C" fn(XmlParser, DataHandler),
    set_default_handler_expand: unsafe extern "C" fn(XmlParser, DataHandler),
    parse: unsafe extern "C" fn(XmlParser, *const c_char, c_int, c_int) -> c_int,
    free: unsafe extern "C" fn(XmlParser),
}

fn expat() -> Option<&'static Expat> {
    static LIB: std::sync::OnceLock<Option<Expat>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| unsafe {
        let h = libc::dlopen(c"libexpat.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if h.is_null() {
            return None;
        }
        let sym = |name: &CStr| {
            let p = libc::dlsym(h, name.as_ptr());
            (!p.is_null()).then_some(p)
        };
        Some(Expat {
            create_ns: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(*const c_char, c_char) -> XmlParser,
            >(sym(c"XML_ParserCreateNS")?),
            set_user_data: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(XmlParser, *mut c_void),
            >(sym(c"XML_SetUserData")?),
            set_element_handler: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(XmlParser, StartHandler, EndHandler),
            >(sym(c"XML_SetElementHandler")?),
            set_character_data_handler: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(XmlParser, DataHandler),
            >(sym(c"XML_SetCharacterDataHandler")?),
            set_default_handler_expand: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(XmlParser, DataHandler),
            >(sym(c"XML_SetDefaultHandlerExpand")?),
            parse: std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(XmlParser, *const c_char, c_int, c_int) -> c_int,
            >(sym(c"XML_Parse")?),
            free: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(XmlParser)>(sym(
                c"XML_ParserFree",
            )?),
        })
    })
    .as_ref()
}

struct Collect {
    elements: Vec<Element>,
    error: bool,
}

/// ElementTree's name for a tag or attribute: "{uri}name" for a namespaced one
fn universal(name: *const c_char) -> String {
    let s = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    if s.contains('}') {
        format!("{{{s}")
    } else {
        s.into_owned()
    }
}

unsafe extern "C" fn on_start(user: *mut c_void, name: *const c_char, atts: *mut *const c_char) {
    let c = &mut *(user as *mut Collect);
    let mut attrs = vec![];
    let mut p = atts;
    while !(*p).is_null() {
        let key = universal(*p);
        let value = CStr::from_ptr(*p.add(1)).to_string_lossy().into_owned();
        attrs.push((key, value));
        p = p.add(2);
    }
    c.elements.push((universal(name), attrs));
}

unsafe extern "C" fn on_end(_: *mut c_void, _: *const c_char) {}

unsafe extern "C" fn on_text(_: *mut c_void, _: *const c_char, _: c_int) {}

/// What expat hands on unhandled; an entity reference here is one it could not expand: ElementTree's "undefined entity".
unsafe extern "C" fn on_default(user: *mut c_void, s: *const c_char, len: c_int) {
    let c = &mut *(user as *mut Collect);
    if len >= 2 && *s as u8 == b'&' {
        c.error = true;
    }
}

/// The elements of a plain XML document in document order (ElementTree's root.iter()), or Err where ElementTree raised a
/// ParseError.
pub fn parse(data: &[u8]) -> Result<Vec<Element>, Unparsable> {
    let Some(x) = expat() else {
        crate::crash(
            "liki-icon-import",
            "libexpat.so.1 could not be opened (it reads plain appfilter.xml files)",
        );
    };
    let mut collect = Collect {
        elements: vec![],
        error: false,
    };
    let ok = unsafe {
        let p = (x.create_ns)(std::ptr::null(), b'}' as c_char);
        if p.is_null() {
            return Err(Unparsable);
        }
        (x.set_user_data)(p, &mut collect as *mut Collect as *mut c_void);
        (x.set_element_handler)(p, on_start, on_end);
        (x.set_character_data_handler)(p, on_text);
        (x.set_default_handler_expand)(p, on_default);
        // feed(data) then close(), as ElementTree.fromstring does
        let len = c_int::try_from(data.len()).unwrap_or(c_int::MAX);
        let first = (x.parse)(p, data.as_ptr() as *const c_char, len, 0);
        let last = if first == 1 {
            (x.parse)(p, std::ptr::null(), 0, 1)
        } else {
            0
        };
        (x.free)(p);
        first == 1 && last == 1
    };
    if !ok || collect.error {
        return Err(Unparsable);
    }
    Ok(collect.elements)
}
