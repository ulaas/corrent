use super::didl::{self, Object};
use super::ssdp;

#[test]
fn reads_soap_fields() {
    let body = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body>
        <u:Browse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1"><ObjectID>i42</ObjectID>
        <BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>10</StartingIndex>
        <RequestedCount>0</RequestedCount><SortCriteria/></u:Browse></s:Body></s:Envelope>"#;
    assert_eq!(didl::tag(body, "ObjectID").as_deref(), Some("i42"));
    assert_eq!(didl::tag(body, "BrowseFlag").as_deref(), Some("BrowseDirectChildren"));
    assert_eq!(didl::tag(body, "StartingIndex").as_deref(), Some("10"));
    assert_eq!(didl::tag(body, "SortCriteria").as_deref(), Some(""));
    assert_eq!(didl::tag(body, "Missing"), None);
    assert_eq!(didl::tag("<a:ObjectID>x&amp;y</a:ObjectID>", "ObjectID").as_deref(), Some("x&y"));
}

#[test]
fn writes_escaped_didl() {
    let d = didl::document(&[
        Object::Container { id: "l1".into(), parent: "0".into(), title: "Films & <Shows>".into(), children: Some(3) },
        Object::Item(didl::Item {
            id: "i7".into(),
            parent: "l1".into(),
            title: "Sintel".into(),
            class: "object.item.videoItem.movie",
            year: Some(2010),
            artist: None,
            album: None,
            track: None,
            duration_ms: Some(888_000),
            art: None,
            url: "http://192.168.1.2:8201/dlna/media/7/ab.mkv".into(),
            mime: "video/x-matroska".into(),
            size: 123,
        }),
    ]);
    assert!(d.contains("Films &amp; &lt;Shows&gt;"));
    assert!(d.contains(r#"childCount="3""#));
    assert!(d.contains(r#"duration="0:14:48.000""#));
    assert!(d.contains("<dc:date>2010-01-01</dc:date>"));
    assert!(d.contains(r#"protocolInfo="http-get:*:video/x-matroska:DLNA.ORG_OP=01;DLNA.ORG_CI=0" size="123""#));
}

#[test]
fn answers_searches() {
    let udn = "uuid:1234";
    let m = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n";
    let st = ssdp::search_target(m).unwrap();
    assert_eq!(ssdp::answers(&st, udn), vec![super::DEVICE.to_string()]);
    assert_eq!(ssdp::answers("ssdp:all", udn).len(), 5);
    assert!(ssdp::answers("urn:schemas-upnp-org:device:MediaRenderer:1", udn).is_empty());
    // Without the discover MAN header, or not a search at all.
    assert_eq!(ssdp::search_target(b"M-SEARCH * HTTP/1.1\r\nST: ssdp:all\r\n\r\n"), None);
    assert_eq!(ssdp::search_target(b"NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\n\r\n"), None);
    assert_eq!(ssdp::search_target(&[0xff, 0xfe]), None);
}

#[test]
fn stable_uuid() {
    let a = super::udn("server-a");
    assert_eq!(a, super::udn("server-a"));
    assert_ne!(a, super::udn("server-b"));
    assert!(a.starts_with("uuid:") && a.len() == 5 + 36, "{a}");
}
