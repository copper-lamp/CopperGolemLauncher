// SPDX-License-Identifier: GPL-3.0-only
//
// Windows RST / WS-Trust transport ported to Rust from LeviLauncher
// `internal/nativeinstall` (`rst_windows.go`) and `internal/xbox`
// (`auth_windows.go`).
//
// The RST protocol, the `RST2.srf` report layout and the response validation
// order are adapted from Xodus commit
// 0670e25aeb0e0e9f800f8f2f4968ae3b681842a7 (GPL-3.0-only). The CLEP key
// schedule credited there to LukeFZ (MIT-licensed SPLicense work) is not
// duplicated here; `store_rst` already owns it.

//! RST / WS-Trust transport for `https://login.live.com/RST2.srf`.
//!
//! The module owns the wire format only:
//!
//! * [`XmlDocument`] – a real XML tree with exclusive XML canonicalization 1.0
//!   (`xml-exc-c14n#`, empty `InclusiveNamespaces` prefix list), used both to
//!   build request digests and to verify response digests;
//! * [`make_rst`] – builds the `s:Envelope` request (AuthInfo, Security,
//!   Timestamp, RequestSecurityToken, Signature and its three references),
//!   signed with RSA-PKCS1v15(SHA-256) or HMAC-SHA256 over a WS-SecureConversation
//!   double-derived key;
//! * [`post_rst`] – performs the bounded POST exchange and parses the response;
//! * [`decrypt_rst_response`] – verifies the response signature and every
//!   referenced digest, then decrypts `EncryptedData` payloads.
//!
//! Credentials never originate here: member names, tickets, record keys and
//! secrets are caller-provided values (DPAPI storage lives in [`store_device`],
//! the DPAPI/WAM boundary in [`store_wam`]), and nothing in this module mints,
//! invents or persists a token. `RstDocument` and `RstResponse` deliberately
//! implement `Debug` by hand so a ticket or a token cannot leak through logging.
//!
//! [`store_rst`]: crate::services::store_rst
//! [`store_device`]: crate::services::store_device
//! [`store_wam`]: crate::services::store_wam

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use openssl::hash::MessageDigest;
use openssl::memcmp;
use openssl::pkey::{PKey, Private};
use openssl::rand::rand_bytes;
use openssl::rsa::Rsa;
use openssl::sign::Signer;
use openssl::symm::{Cipher, Crypter, Mode};
use quick_xml::encoding::Decoder;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use reqwest::Client;
use sha2::{Digest, Sha256};

use crate::services::store_rst::{StoreRstError, cbc_decrypt, decrypt_clep};

// ---------------------------------------------------------------------------
// Protocol constants (kept byte-identical to the reference implementation)
// ---------------------------------------------------------------------------

/// HTTPS endpoint receiving the canonicalized RST envelope.
pub const RST_ENDPOINT: &str = "https://login.live.com/RST2.srf";
/// `wsa:To` value, including the explicit `:443` used by the reference client.
pub const RST_TO: &str = "https://login.live.com:443/RST2.srf";
/// `wsa:Action` value for an RST issue request.
pub const RST_ACTION: &str = "http://schemas.xmlsoap.org/ws/2005/02/trust/RST/Issue";
/// `wst:RequestType` value for an RST issue request.
pub const RST_REQUEST_TYPE: &str = "http://schemas.xmlsoap.org/ws/2005/02/trust/Issue";
/// User agent required by the MSA endpoint.
pub const RST_USER_AGENT: &str = "MSAWindows/55";
/// Content type of the RST exchange.
pub const RST_CONTENT_TYPE: &str = "application/soap+xml";
/// Hard response ceiling (2 MiB), matching the reference's bounded body reader.
pub const RST_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
/// Length of the client nonce embedded in the `wssc:DerivedKeyToken`.
pub const RST_NONCE_LENGTH: usize = 32;
/// Lifetime of the `wsu:Timestamp` window, in seconds.
pub const RST_TIMESTAMP_LIFETIME: u64 = 300;
/// Relying-party scope that suppresses the `wsp:PolicyReference` element.
pub const TOKEN_BROKER_SCOPE: &str = "http://Passport.NET/tb";

/// `SignatureMethod` for RSA-PKCS1v15 over SHA-256.
pub const SIGNATURE_METHOD_RSA_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
/// `SignatureMethod` for HMAC-SHA256 over the derived key.
pub const SIGNATURE_METHOD_HMAC_SHA256: &str = "http://www.w3.org/2001/04/xmldsig-more#hmac-sha256";
/// XML digital signature namespace.
pub const DSIG_NAMESPACE: &str = "http://www.w3.org/2000/09/xmldsig#";
/// Exclusive canonicalization algorithm URI.
pub const EXCLUSIVE_C14N_NAMESPACE: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
/// XML encryption namespace.
pub const XMLENC_NAMESPACE: &str = "http://www.w3.org/2001/04/xmlenc#";
/// The implicitly bound `xml` prefix namespace; never rendered as a declaration.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// `DigestMethod` algorithm URI used for every reference.
pub const DIGEST_METHOD_SHA256: &str = "http://www.w3.org/2001/04/xmlenc#sha256";
/// Live-ID double-derived key algorithm (SP800-108 CTR, HMAC-SHA256).
pub const DERIVED_KEY_ALGORITHM: &str = "urn:liveid:SP800108_CTR_HMAC_SHA256_DOUBLEDERIVED";
/// `ValueType` of the requested-token key identifier.
pub const KEY_IDENTIFIER_VALUE_TYPE: &str = "http://docs.oasis-open.org/wss/2004/XX/oasis-2004XX-wss-saml-token-profile-1.0#SAMLAssertionID";

const NS_SOAP_ENVELOPE: &str = "http://www.w3.org/2003/05/soap-envelope";
const NS_PASSPORT: &str = "http://schemas.microsoft.com/Passport/SoapServices/PPCRL";
const NS_WSSE: &str =
    "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd";
const NS_WSU: &str =
    "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd";
const NS_WSA: &str = "http://www.w3.org/2005/08/addressing";
const NS_WSSC: &str = "http://schemas.xmlsoap.org/ws/2005/02/sc";
const NS_WST: &str = "http://schemas.xmlsoap.org/ws/2005/02/trust";
const NS_WSP: &str = "http://schemas.xmlsoap.org/ws/2004/09/policy";
const NS_SAML: &str = "urn:oasis:names:tc:SAML:1.0:assertion";

/// Envelope namespace declarations, byte-identical to the reference map.
const ENVELOPE_NAMESPACES: [(&str, &str); 9] = [
    ("s", NS_SOAP_ENVELOPE),
    ("ps", NS_PASSPORT),
    ("wsse", NS_WSSE),
    ("wsu", NS_WSU),
    ("wsa", NS_WSA),
    ("wssc", NS_WSSC),
    ("wst", NS_WST),
    ("wsp", NS_WSP),
    ("saml", NS_SAML),
];

/// `ps:HostingApp` for a device (non-SSO) request.
const HOSTING_APP_DEVICE: &str = "{DF60E2DF-88AD-4526-AE21-83D130EF0F68}";
/// `ps:HostingApp` for an SSO request carrying a ticket.
const HOSTING_APP_SSO: &str = "{d6d5a677-0872-4ab0-9442-bb792fce85c5}";
/// `ps:SSOFlags` for an SSO request; empty for a device request.
const SSO_FLAGS: &str = "SsoRestr";

/// `ps:AuthInfo` children that do not depend on the request mode, in order.
const AUTH_INFO_FIELDS: [(&str, &str); 9] = [
    ("BinaryVersion", "55"),
    ("UIVersion", "1"),
    ("InlineUX", "TokenBroker"),
    ("IsAdmin", "1"),
    ("Cookies", ""),
    ("RequestParams", "AQAAAAIAAABsYwQAAAAxMDMz"),
    (
        "WindowsClientString",
        "b4d/QB7Zy5pjUAY9ByQ1echTyTITx6ZCErOEztuIVtw=",
    ),
    ("LicenseSignatureKeyVersion", "2"),
    ("ClientCapabilities", "1"),
];

/// Largest XML payload this module will parse (requests and responses).
const MAX_XML_BYTES: usize = 8 * 1024 * 1024;
/// Nesting ceiling; keeps the recursive tree walks and canonicalizer bounded.
const MAX_XML_DEPTH: usize = 64;
/// AES block size, used for the CBC IV and for PKCS#7 style padding checks.
const AES_BLOCK_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything that can go wrong while building, sending or validating an RST
/// exchange. No variant carries key material or token text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RstTransportError {
    #[error("XML document is malformed")]
    MalformedXml,
    #[error("XML document is empty or exceeds the size limit")]
    XmlTooLarge,
    #[error("XML nesting exceeds the depth limit")]
    XmlTooDeep,
    #[error("XML namespace prefix is not bound in scope")]
    UnboundNamespacePrefix,
    #[error("XML processing instructions inside an element are not supported")]
    UnsupportedProcessingInstruction,
    #[error("expected exactly one {0} element, found {1}")]
    ElementCount(&'static str, usize),
    #[error("RST scope must not be empty")]
    EmptyScope,
    #[error("RST request has no signing key")]
    MissingSigningKey,
    #[error("RST derived key requires a non-empty secret")]
    EmptySecret,
    #[error("RST client nonce must be {RST_NONCE_LENGTH} bytes")]
    InvalidNonce,
    #[error("system clock is outside the supported range")]
    InvalidClock,
    #[error("cryptographic operation failed")]
    Crypto,
    #[error("base64 value is malformed")]
    MalformedBase64,
    #[error("RST request failed: {0}")]
    Http(String),
    #[error("RST response exceeds the size limit")]
    ResponseTooLarge,
    #[error("RST response is unsigned but reports success")]
    UnsignedSuccessResponse,
    #[error("RST signature nonce is missing")]
    MissingSignatureNonce,
    #[error("RST response signature verification failed")]
    SignatureMismatch,
    #[error("RST response contains an ambiguous or missing signed target")]
    AmbiguousSignedTarget,
    #[error("RST signed digest verification failed")]
    SignedDigestMismatch,
    #[error("RST derived key token has no identifier")]
    MissingDerivedKeyId,
    #[error("RST encryption nonce is missing")]
    MissingEncryptionNonce,
    #[error("RST ciphertext is too short")]
    ShortCiphertext,
    #[error("RST ciphertext length is not a multiple of the AES block size")]
    InvalidCiphertextLength,
    #[error("invalid RST padding")]
    InvalidPadding,
    #[error("unsupported RST encryption algorithm")]
    UnsupportedEncryption,
}

/// Map a transport failure onto the caller-facing error type used by the device
/// ticket flow. Cryptographic failures stay distinguishable; every other failure
/// (HTTP, XML, signature, digest, decryption shape) keeps its own message.
impl From<RstTransportError> for StoreRstError {
    fn from(error: RstTransportError) -> Self {
        match error {
            RstTransportError::Crypto | RstTransportError::MalformedBase64 => {
                StoreRstError::Crypto
            }
            other => StoreRstError::Exchange(other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// XML document: parsing, tree access, exclusive canonicalization 1.0
// ---------------------------------------------------------------------------

/// Handle to one element inside an [`XmlDocument`] arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct XmlNode(usize);

#[derive(Debug, Clone, PartialEq, Eq)]
struct XmlAttribute {
    prefix: String,
    local: String,
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum XmlChild {
    Element(usize),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XmlElement {
    prefix: String,
    local: String,
    namespaces: Vec<(String, String)>,
    attributes: Vec<XmlAttribute>,
    children: Vec<XmlChild>,
    parent: Option<usize>,
}

/// A parsed or programmatically built XML tree.
///
/// The tree keeps parent links, so an element can be canonicalized with the
/// namespace declarations it inherits from its ancestors – exactly what a
/// `Reference URI="#fragment"` digest requires.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XmlDocument {
    elements: Vec<XmlElement>,
}

impl XmlDocument {
    /// Create an empty document; the first element created becomes the root.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse UTF-8 XML into a tree.
    ///
    /// Line endings are normalized (`\r\n` and `\r` become `\n`) before parsing,
    /// as XML processors are required to do; everything else, including
    /// whitespace-only text nodes, is preserved byte for byte so that
    /// canonicalization reproduces what the peer signed.
    pub fn parse(xml: &[u8]) -> Result<Self, RstTransportError> {
        if xml.is_empty() {
            return Err(RstTransportError::MalformedXml);
        }
        if xml.len() > MAX_XML_BYTES {
            return Err(RstTransportError::XmlTooLarge);
        }
        let normalized = normalize_line_endings(xml);
        let mut reader = Reader::from_reader(normalized.as_ref());
        let mut document = Self::default();
        let mut stack: Vec<XmlNode> = Vec::new();
        let mut buffer: Vec<u8> = Vec::new();
        let mut root: Option<XmlNode> = None;
        let mut root_closed = false;
        loop {
            let event = reader
                .read_event_into(&mut buffer)
                .map_err(|_| RstTransportError::MalformedXml)?;
            match event {
                Event::Start(start) => {
                    if root_closed {
                        return Err(RstTransportError::MalformedXml);
                    }
                    if stack.len() >= MAX_XML_DEPTH {
                        return Err(RstTransportError::XmlTooDeep);
                    }
                    let parent = stack.last().copied();
                    let node =
                        document.push_element(&start, reader.decoder(), parent.map(|node| node.0))?;
                    if parent.is_none() && root.replace(node).is_some() {
                        return Err(RstTransportError::MalformedXml);
                    }
                    stack.push(node);
                }
                Event::Empty(start) => {
                    if root_closed {
                        return Err(RstTransportError::MalformedXml);
                    }
                    if stack.len() >= MAX_XML_DEPTH {
                        return Err(RstTransportError::XmlTooDeep);
                    }
                    let parent = stack.last().copied();
                    let node =
                        document.push_element(&start, reader.decoder(), parent.map(|node| node.0))?;
                    if parent.is_none() {
                        if root.replace(node).is_some() {
                            return Err(RstTransportError::MalformedXml);
                        }
                        // An empty root element is complete as soon as it closes.
                        root_closed = true;
                    }
                }
                Event::End(_) => {
                    if stack.pop().is_none() {
                        return Err(RstTransportError::MalformedXml);
                    }
                    if stack.is_empty() {
                        root_closed = true;
                    }
                }
                Event::Text(text) => {
                    let decoded = text
                        .decode()
                        .map_err(|_| RstTransportError::MalformedXml)?
                        .into_owned();
                    document.push_text(stack.last().copied(), decoded)?;
                }
                Event::CData(data) => {
                    let decoded = data
                        .decode()
                        .map_err(|_| RstTransportError::MalformedXml)?
                        .into_owned();
                    document.push_text(stack.last().copied(), decoded)?;
                }
                Event::GeneralRef(reference) => {
                    let resolved = resolve_reference(&reference)?;
                    document.push_text(stack.last().copied(), resolved)?;
                }
                Event::PI(_) => {
                    // Processing instructions outside the root are prolog noise;
                    // inside an element they would belong in canonical output,
                    // which this module never needs to emit.
                    if !stack.is_empty() {
                        return Err(RstTransportError::UnsupportedProcessingInstruction);
                    }
                }
                Event::Decl(_) | Event::Comment(_) | Event::DocType(_) => {}
                Event::Eof => break,
            }
            buffer.clear();
        }
        if root.is_none() || !root_closed || !stack.is_empty() {
            return Err(RstTransportError::MalformedXml);
        }
        Ok(document)
    }

    /// Root element. Only valid for documents that contain one.
    pub fn root(&self) -> XmlNode {
        XmlNode(0)
    }

    /// Create a detached element from a qualified name (`prefix:local`).
    pub fn create_element(
        &mut self,
        qualified_name: &str,
    ) -> Result<XmlNode, RstTransportError> {
        let (prefix, local) = split_name(qualified_name)?;
        let index = self.elements.len();
        self.elements.push(XmlElement {
            prefix,
            local,
            namespaces: Vec::new(),
            attributes: Vec::new(),
            children: Vec::new(),
            parent: None,
        });
        Ok(XmlNode(index))
    }

    /// Create a child element and attach it to `parent`.
    pub fn append_element(
        &mut self,
        parent: XmlNode,
        qualified_name: &str,
    ) -> Result<XmlNode, RstTransportError> {
        let node = self.create_element(qualified_name)?;
        self.attach(parent, node)?;
        Ok(node)
    }

    /// Create a child element holding a single text node.
    pub fn append_text_element(
        &mut self,
        parent: XmlNode,
        qualified_name: &str,
        text: &str,
    ) -> Result<XmlNode, RstTransportError> {
        let node = self.append_element(parent, qualified_name)?;
        if !text.is_empty() {
            self.push_text(Some(node), text.to_string())?;
        }
        Ok(node)
    }

    /// Append a text node to `parent` (no-op without a parent element).
    pub fn append_text(&mut self, parent: XmlNode, text: &str) -> Result<(), RstTransportError> {
        self.push_text(Some(parent), text.to_string())
    }

    /// Replace the direct text children of `node` with a single text node.
    pub fn set_text(&mut self, node: XmlNode, text: &str) -> Result<(), RstTransportError> {
        let element = self
            .elements
            .get_mut(node.0)
            .ok_or(RstTransportError::MalformedXml)?;
        element
            .children
            .retain(|child| !matches!(child, XmlChild::Text(_)));
        self.push_text(Some(node), text.to_string())
    }

    /// Declare a namespace binding on `node`; an existing binding with the same
    /// prefix is replaced, mirroring attribute creation in the reference client.
    pub fn declare_namespace(
        &mut self,
        node: XmlNode,
        prefix: &str,
        uri: &str,
    ) -> Result<(), RstTransportError> {
        let element = self
            .elements
            .get_mut(node.0)
            .ok_or(RstTransportError::MalformedXml)?;
        match element
            .namespaces
            .iter_mut()
            .find(|(existing, _)| existing == prefix)
        {
            Some((_, existing_uri)) => *existing_uri = uri.to_string(),
            None => element.namespaces.push((prefix.to_string(), uri.to_string())),
        }
        Ok(())
    }

    /// Set an attribute, replacing an existing one with the same qualified
    /// name. `xmlns` and `xmlns:prefix` are routed to [`Self::declare_namespace`].
    pub fn set_attribute(
        &mut self,
        node: XmlNode,
        qualified_name: &str,
        value: &str,
    ) -> Result<(), RstTransportError> {
        if qualified_name == "xmlns" {
            return self.declare_namespace(node, "", value);
        }
        if let Some(prefix) = qualified_name.strip_prefix("xmlns:") {
            return self.declare_namespace(node, prefix, value);
        }
        let (prefix, local) = split_name(qualified_name)?;
        let element = self
            .elements
            .get_mut(node.0)
            .ok_or(RstTransportError::MalformedXml)?;
        match element
            .attributes
            .iter_mut()
            .find(|existing| existing.prefix == prefix && existing.local == local)
        {
            Some(existing) => existing.value = value.to_string(),
            None => element.attributes.push(XmlAttribute {
                prefix,
                local,
                value: value.to_string(),
            }),
        }
        Ok(())
    }

    /// Deep-copy the subtree rooted at `source_root` into this document under
    /// `parent`.
    pub fn append_subtree(
        &mut self,
        parent: XmlNode,
        source: &XmlDocument,
        source_root: XmlNode,
    ) -> Result<XmlNode, RstTransportError> {
        self.copy_subtree(parent.0, source, source_root, true)
    }

    /// Replace the child `existing` of `parent` with a copy of `replacement`
    /// taken from another document, keeping the original child position.
    pub fn replace_child(
        &mut self,
        parent: XmlNode,
        existing: XmlNode,
        replacement: &XmlDocument,
        replacement_root: XmlNode,
    ) -> Result<(), RstTransportError> {
        let position = self
            .elements
            .get(parent.0)
            .ok_or(RstTransportError::MalformedXml)?
            .children
            .iter()
            .position(|child| matches!(child, XmlChild::Element(index) if *index == existing.0))
            .ok_or(RstTransportError::MalformedXml)?;
        // The copy is not attached here: it takes over the existing slot.
        let inserted = self.copy_subtree(parent.0, replacement, replacement_root, false)?;
        let children = &mut self
            .elements
            .get_mut(parent.0)
            .ok_or(RstTransportError::MalformedXml)?
            .children;
        match children.get_mut(position) {
            Some(slot) => {
                *slot = XmlChild::Element(inserted.0);
                Ok(())
            }
            None => Err(RstTransportError::MalformedXml),
        }
    }

    /// Local name of an element (without its prefix).
    pub fn local_name(&self, node: XmlNode) -> &str {
        self.elements
            .get(node.0)
            .map_or("", |element| element.local.as_str())
    }

    /// Prefix of an element (empty when it has none).
    pub fn prefix(&self, node: XmlNode) -> &str {
        self.elements
            .get(node.0)
            .map_or("", |element| element.prefix.as_str())
    }

    /// Qualified name of an element as written in the document.
    pub fn qualified_name(&self, node: XmlNode) -> String {
        match self.elements.get(node.0) {
            Some(element) if !element.prefix.is_empty() => {
                format!("{}:{}", element.prefix, element.local)
            }
            Some(element) => element.local.clone(),
            None => String::new(),
        }
    }

    /// Value of an attribute addressed by its qualified name.
    pub fn attribute(&self, node: XmlNode, qualified_name: &str) -> Option<&str> {
        let element = self.elements.get(node.0)?;
        let (prefix, local) = match qualified_name.split_once(':') {
            Some((prefix, local)) => (prefix, local),
            None => ("", qualified_name),
        };
        element
            .attributes
            .iter()
            .find(|attribute| attribute.prefix == prefix && attribute.local == local)
            .map(|attribute| attribute.value.as_str())
    }

    /// The identifier used by `Reference URI="#..."`: the unprefixed `Id`
    /// attribute, then `wsu:Id`.
    pub fn element_id(&self, node: XmlNode) -> Option<&str> {
        self.attribute(node, "Id")
            .or_else(|| self.attribute(node, "wsu:Id"))
    }

    /// Concatenated text of an element's direct text children.
    pub fn text(&self, node: XmlNode) -> String {
        let Some(element) = self.elements.get(node.0) else {
            return String::new();
        };
        let mut text = String::new();
        for child in &element.children {
            if let XmlChild::Text(value) = child {
                text.push_str(value);
            }
        }
        text
    }

    /// Parent of an element.
    pub fn parent(&self, node: XmlNode) -> Option<XmlNode> {
        self.elements.get(node.0)?.parent.map(XmlNode)
    }

    /// Direct child elements, in document order.
    pub fn children(&self, node: XmlNode) -> Vec<XmlNode> {
        let Some(element) = self.elements.get(node.0) else {
            return Vec::new();
        };
        element
            .children
            .iter()
            .filter_map(|child| match child {
                XmlChild::Element(index) => Some(XmlNode(*index)),
                XmlChild::Text(_) => None,
            })
            .collect()
    }

    /// The element itself and all of its descendants, in document order.
    pub fn descendants(&self, node: XmlNode) -> Vec<XmlNode> {
        let mut collected = Vec::new();
        self.collect_descendants(node, &mut collected);
        collected
    }

    /// All elements with the given local name at or below `node`, in document
    /// order. Namespace prefixes are ignored, matching the reference walker.
    pub fn find_all(&self, node: XmlNode, local_name: &str) -> Vec<XmlNode> {
        self.descendants(node)
            .into_iter()
            .filter(|candidate| self.local_name(*candidate) == local_name)
            .collect()
    }

    /// Exactly one element with the given local name at or below `node`.
    pub fn find_unique(
        &self,
        node: XmlNode,
        local_name: &'static str,
    ) -> Result<XmlNode, RstTransportError> {
        let matches = self.find_all(node, local_name);
        match matches.len() {
            1 => matches
                .into_iter()
                .next()
                .ok_or(RstTransportError::ElementCount(local_name, 0)),
            count => Err(RstTransportError::ElementCount(local_name, count)),
        }
    }

    /// Exclusive XML canonicalization 1.0 (`xml-exc-c14n#`) with an empty
    /// `InclusiveNamespaces` prefix list.
    ///
    /// The canonicalization is a real implementation rather than a formatter:
    ///
    /// * a subtree sees the namespace declarations of its ancestors, so a
    ///   detached element canonicalizes exactly as it would inside its document;
    /// * only namespace declarations that are visibly utilized here and not
    ///   already emitted by an output ancestor are written, sorted by prefix
    ///   with the default namespace first;
    /// * attributes sort by namespace URI then local name, with attributes in
    ///   no namespace first (sorted by local name);
    /// * text escapes `&`, `<`, `>` and `\r`; attribute values escape `&`, `<`,
    ///   `"`, `\t`, `\n` and `\r`;
    /// * an element without children is written as `<a></a>` – canonical XML
    ///   never uses the empty-element tag.
    pub fn canonicalize(&self, node: XmlNode) -> Result<Vec<u8>, RstTransportError> {
        let inherited = self.inherited_namespaces(node)?;
        let mut output = Vec::new();
        self.write_canonical(node, &inherited, &BTreeMap::new(), &mut output)?;
        Ok(output)
    }

    /// Canonical form of the whole document.
    pub fn canonicalize_root(&self) -> Result<Vec<u8>, RstTransportError> {
        if self.elements.is_empty() {
            return Err(RstTransportError::MalformedXml);
        }
        self.canonicalize(self.root())
    }

    /// Plain (non-canonical) serialization of a subtree, as written.
    pub fn to_xml(&self, node: XmlNode) -> Result<String, RstTransportError> {
        let mut output = Vec::new();
        self.write_xml(node, &mut output)?;
        String::from_utf8(output).map_err(|_| RstTransportError::MalformedXml)
    }

    fn attach(&mut self, parent: XmlNode, child: XmlNode) -> Result<(), RstTransportError> {
        let parent_element = self
            .elements
            .get_mut(parent.0)
            .ok_or(RstTransportError::MalformedXml)?;
        parent_element.children.push(XmlChild::Element(child.0));
        let child_element = self
            .elements
            .get_mut(child.0)
            .ok_or(RstTransportError::MalformedXml)?;
        child_element.parent = Some(parent.0);
        Ok(())
    }

    fn push_text(
        &mut self,
        parent: Option<XmlNode>,
        text: String,
    ) -> Result<(), RstTransportError> {
        let Some(parent) = parent else {
            return Ok(());
        };
        let element = self
            .elements
            .get_mut(parent.0)
            .ok_or(RstTransportError::MalformedXml)?;
        element.children.push(XmlChild::Text(text));
        Ok(())
    }

    fn push_element(
        &mut self,
        start: &BytesStart<'_>,
        decoder: Decoder,
        parent: Option<usize>,
    ) -> Result<XmlNode, RstTransportError> {
        let raw_name = start.name();
        let name = std::str::from_utf8(raw_name.as_ref())
            .map_err(|_| RstTransportError::MalformedXml)?;
        let (prefix, local) = split_name(name)?;
        let mut namespaces: Vec<(String, String)> = Vec::new();
        let mut attributes: Vec<XmlAttribute> = Vec::new();
        for attribute in start.attributes().with_checks(true) {
            let attribute = attribute.map_err(|_| RstTransportError::MalformedXml)?;
            let key = std::str::from_utf8(attribute.key.as_ref())
                .map_err(|_| RstTransportError::MalformedXml)?;
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                .map_err(|_| RstTransportError::MalformedXml)?
                .into_owned();
            if key == "xmlns" {
                upsert_namespace(&mut namespaces, "", &value);
            } else if let Some(namespace_prefix) = key.strip_prefix("xmlns:") {
                if namespace_prefix.is_empty() {
                    return Err(RstTransportError::MalformedXml);
                }
                upsert_namespace(&mut namespaces, namespace_prefix, &value);
            } else {
                let (attribute_prefix, attribute_local) = split_name(key)?;
                attributes.push(XmlAttribute {
                    prefix: attribute_prefix,
                    local: attribute_local,
                    value,
                });
            }
        }
        let index = self.elements.len();
        self.elements.push(XmlElement {
            prefix,
            local,
            namespaces,
            attributes,
            children: Vec::new(),
            parent,
        });
        let node = XmlNode(index);
        if let Some(parent) = parent {
            self.attach(XmlNode(parent), node)?;
        }
        Ok(node)
    }

    fn collect_descendants(&self, node: XmlNode, collected: &mut Vec<XmlNode>) {
        let Some(element) = self.elements.get(node.0) else {
            return;
        };
        collected.push(node);
        for child in &element.children {
            if let XmlChild::Element(index) = child {
                self.collect_descendants(XmlNode(*index), collected);
            }
        }
    }

    fn copy_subtree(
        &mut self,
        parent: usize,
        source: &XmlDocument,
        source_node: XmlNode,
        attach_to_parent: bool,
    ) -> Result<XmlNode, RstTransportError> {
        let element = source
            .elements
            .get(source_node.0)
            .ok_or(RstTransportError::MalformedXml)?
            .clone();
        let index = self.elements.len();
        self.elements.push(XmlElement {
            prefix: element.prefix,
            local: element.local,
            namespaces: element.namespaces,
            attributes: element.attributes,
            children: Vec::new(),
            parent: Some(parent),
        });
        for child in &element.children {
            match child {
                XmlChild::Text(text) => {
                    let text = text.clone();
                    self.push_text(Some(XmlNode(index)), text)?;
                }
                XmlChild::Element(child_index) => {
                    self.copy_subtree(index, source, XmlNode(*child_index), true)?;
                }
            }
        }
        if attach_to_parent {
            let parent_element = self
                .elements
                .get_mut(parent)
                .ok_or(RstTransportError::MalformedXml)?;
            parent_element.children.push(XmlChild::Element(index));
        }
        Ok(XmlNode(index))
    }

    /// Namespace bindings in scope for `node`, including its own declarations,
    /// collected from the root downwards so inner declarations win.
    fn inherited_namespaces(
        &self,
        node: XmlNode,
    ) -> Result<BTreeMap<String, String>, RstTransportError> {
        let mut chain = Vec::new();
        let mut cursor = Some(node.0);
        while let Some(index) = cursor {
            let element = self
                .elements
                .get(index)
                .ok_or(RstTransportError::MalformedXml)?;
            chain.push(index);
            cursor = element.parent;
            if chain.len() > self.elements.len() {
                return Err(RstTransportError::MalformedXml);
            }
        }
        chain.reverse();
        let mut scope = BTreeMap::new();
        for index in chain {
            if let Some(element) = self.elements.get(index) {
                for (prefix, uri) in &element.namespaces {
                    scope.insert(prefix.clone(), uri.clone());
                }
            }
        }
        Ok(scope)
    }

    fn write_canonical(
        &self,
        node: XmlNode,
        inherited: &BTreeMap<String, String>,
        rendered: &BTreeMap<String, String>,
        output: &mut Vec<u8>,
    ) -> Result<(), RstTransportError> {
        let element = self
            .elements
            .get(node.0)
            .ok_or(RstTransportError::MalformedXml)?;
        let mut scope = inherited.clone();
        for (prefix, uri) in &element.namespaces {
            scope.insert(prefix.clone(), uri.clone());
        }
        if !element.prefix.is_empty() && !scope.contains_key(&element.prefix) {
            return Err(RstTransportError::UnboundNamespacePrefix);
        }
        // Attribute namespaces are resolved here because the exclusive rule
        // depends on which prefixes the attributes visibly use.
        let mut attributes: Vec<(&str, &str, &str, &str)> =
            Vec::with_capacity(element.attributes.len());
        for attribute in &element.attributes {
            let uri = if attribute.prefix.is_empty() {
                ""
            } else if attribute.prefix == "xml" {
                // The `xml` prefix is bound implicitly and never declared.
                XML_NAMESPACE
            } else {
                scope
                    .get(&attribute.prefix)
                    .map(String::as_str)
                    .ok_or(RstTransportError::UnboundNamespacePrefix)?
            };
            attributes.push((
                uri,
                attribute.local.as_str(),
                attribute.prefix.as_str(),
                attribute.value.as_str(),
            ));
        }
        attributes.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));

        let mut nested_rendered = rendered.clone();
        let mut declarations: Vec<(&str, &str)> = Vec::new();
        for (prefix, uri) in &scope {
            let utilized = *prefix == element.prefix
                || attributes
                    .iter()
                    .any(|(_, _, attribute_prefix, _)| {
                        !attribute_prefix.is_empty() && *attribute_prefix == prefix.as_str()
                    });
            if !utilized {
                continue;
            }
            let already_rendered = rendered.get(prefix);
            if already_rendered == Some(uri) {
                continue;
            }
            // An empty default namespace only has to be written when it
            // un-declares a mapping emitted by an output ancestor.
            if uri.is_empty() && already_rendered.is_none() {
                continue;
            }
            declarations.push((prefix.as_str(), uri.as_str()));
            nested_rendered.insert(prefix.clone(), uri.clone());
        }

        output.push(b'<');
        write_qualified_name(&element.prefix, &element.local, output);
        for (prefix, uri) in &declarations {
            output.extend_from_slice(b" xmlns");
            if !prefix.is_empty() {
                output.push(b':');
                output.extend_from_slice(prefix.as_bytes());
            }
            output.extend_from_slice(b"=\"");
            escape_attribute_value(uri, output);
            output.push(b'"');
        }
        for (_, local, prefix, value) in &attributes {
            output.push(b' ');
            write_qualified_name(prefix, local, output);
            output.extend_from_slice(b"=\"");
            escape_attribute_value(value, output);
            output.push(b'"');
        }
        if element.children.is_empty() {
            output.extend_from_slice(b"></");
            write_qualified_name(&element.prefix, &element.local, output);
            output.push(b'>');
            return Ok(());
        }
        output.push(b'>');
        for child in &element.children {
            match child {
                XmlChild::Text(text) => escape_text(text, output),
                XmlChild::Element(index) => {
                    self.write_canonical(XmlNode(*index), &scope, &nested_rendered, output)?;
                }
            }
        }
        output.extend_from_slice(b"</");
        write_qualified_name(&element.prefix, &element.local, output);
        output.push(b'>');
        Ok(())
    }

    fn write_xml(&self, node: XmlNode, output: &mut Vec<u8>) -> Result<(), RstTransportError> {
        let element = self
            .elements
            .get(node.0)
            .ok_or(RstTransportError::MalformedXml)?;
        output.push(b'<');
        write_qualified_name(&element.prefix, &element.local, output);
        for (prefix, uri) in &element.namespaces {
            output.extend_from_slice(b" xmlns");
            if !prefix.is_empty() {
                output.push(b':');
                output.extend_from_slice(prefix.as_bytes());
            }
            output.extend_from_slice(b"=\"");
            escape_attribute_value(uri, output);
            output.push(b'"');
        }
        for attribute in &element.attributes {
            output.push(b' ');
            write_qualified_name(&attribute.prefix, &attribute.local, output);
            output.extend_from_slice(b"=\"");
            escape_attribute_value(&attribute.value, output);
            output.push(b'"');
        }
        if element.children.is_empty() {
            output.extend_from_slice(b"/>");
            return Ok(());
        }
        output.push(b'>');
        for child in &element.children {
            match child {
                XmlChild::Text(text) => escape_text(text, output),
                XmlChild::Element(index) => self.write_xml(XmlNode(*index), output)?,
            }
        }
        output.extend_from_slice(b"</");
        write_qualified_name(&element.prefix, &element.local, output);
        output.push(b'>');
        Ok(())
    }
}

fn upsert_namespace(namespaces: &mut Vec<(String, String)>, prefix: &str, uri: &str) {
    match namespaces
        .iter_mut()
        .find(|(existing, _)| existing == prefix)
    {
        Some((_, existing_uri)) => *existing_uri = uri.to_string(),
        None => namespaces.push((prefix.to_string(), uri.to_string())),
    }
}

fn split_name(qualified_name: &str) -> Result<(String, String), RstTransportError> {
    match qualified_name.split_once(':') {
        Some((prefix, local)) => {
            if prefix.is_empty() || local.is_empty() || local.contains(':') {
                return Err(RstTransportError::MalformedXml);
            }
            Ok((prefix.to_string(), local.to_string()))
        }
        None if qualified_name.is_empty() => Err(RstTransportError::MalformedXml),
        None => Ok((String::new(), qualified_name.to_string())),
    }
}

fn normalize_line_endings(xml: &[u8]) -> Cow<'_, [u8]> {
    if !xml.contains(&b'\r') {
        return Cow::Borrowed(xml);
    }
    let mut normalized = Vec::with_capacity(xml.len());
    let mut index = 0usize;
    while index < xml.len() {
        match xml.get(index) {
            Some(b'\r') => {
                normalized.push(b'\n');
                index += 1;
                if xml.get(index) == Some(&b'\n') {
                    index += 1;
                }
            }
            Some(byte) => {
                normalized.push(*byte);
                index += 1;
            }
            None => break,
        }
    }
    Cow::Owned(normalized)
}

fn resolve_reference(
    reference: &quick_xml::events::BytesRef<'_>,
) -> Result<String, RstTransportError> {
    if reference.is_char_ref() {
        return reference
            .resolve_char_ref()
            .map_err(|_| RstTransportError::MalformedXml)?
            .map(|character| character.to_string())
            .ok_or(RstTransportError::MalformedXml);
    }
    let name = std::str::from_utf8(reference.as_ref()).map_err(|_| RstTransportError::MalformedXml)?;
    resolve_predefined_entity(name)
        .map(str::to_string)
        .ok_or(RstTransportError::MalformedXml)
}

fn write_qualified_name(prefix: &str, local: &str, output: &mut Vec<u8>) {
    if !prefix.is_empty() {
        output.extend_from_slice(prefix.as_bytes());
        output.push(b':');
    }
    output.extend_from_slice(local.as_bytes());
}

/// Canonical text escaping: `&`, `<`, `>` and `\r`.
fn escape_text(text: &str, output: &mut Vec<u8>) {
    for character in text.chars() {
        match character {
            '&' => output.extend_from_slice(b"&amp;"),
            '<' => output.extend_from_slice(b"&lt;"),
            '>' => output.extend_from_slice(b"&gt;"),
            '\r' => output.extend_from_slice(b"&#xD;"),
            other => push_character(other, output),
        }
    }
}

/// Canonical attribute escaping: `&`, `<`, `"`, `\t`, `\n` and `\r`.
fn escape_attribute_value(value: &str, output: &mut Vec<u8>) {
    for character in value.chars() {
        match character {
            '&' => output.extend_from_slice(b"&amp;"),
            '<' => output.extend_from_slice(b"&lt;"),
            '"' => output.extend_from_slice(b"&quot;"),
            '\t' => output.extend_from_slice(b"&#x9;"),
            '\n' => output.extend_from_slice(b"&#xA;"),
            '\r' => output.extend_from_slice(b"&#xD;"),
            other => push_character(other, output),
        }
    }
}

fn push_character(character: char, output: &mut Vec<u8>) {
    let mut buffer = [0u8; 4];
    output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
}

// ---------------------------------------------------------------------------
// Cryptography primitives
// ---------------------------------------------------------------------------

/// WS-SecureConversation double-derived HMAC-SHA256 key.
///
/// [`store_rst::derived_key`] owns the derivation; this wrapper only maps the
/// error type so callers of this module see a single error enum.
///
/// [`store_rst::derived_key`]: crate::services::store_rst::derived_key
pub fn derived(secret: &[u8], nonce: &[u8]) -> Result<[u8; 32], RstTransportError> {
    crate::services::store_rst::derived_key(secret, nonce)
        .map_err(|_| RstTransportError::Crypto)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<Vec<u8>, RstTransportError> {
    let key = PKey::hmac(key).map_err(|_| RstTransportError::Crypto)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)
        .map_err(|_| RstTransportError::Crypto)?;
    signer
        .update(data)
        .map_err(|_| RstTransportError::Crypto)?;
    signer.sign_to_vec().map_err(|_| RstTransportError::Crypto)
}

/// RSA PKCS#1 v1.5 signature over SHA-256, matching `rsa.SignPKCS1v15`.
fn rsa_sign(key: &PKey<Private>, data: &[u8]) -> Result<Vec<u8>, RstTransportError> {
    let mut signer =
        Signer::new(MessageDigest::sha256(), key).map_err(|_| RstTransportError::Crypto)?;
    signer
        .update(data)
        .map_err(|_| RstTransportError::Crypto)?;
    signer.sign_to_vec().map_err(|_| RstTransportError::Crypto)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len() && memcmp::eq(left, right)
}

fn decode_base64(value: &str) -> Result<Vec<u8>, RstTransportError> {
    base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .map_err(|_| RstTransportError::MalformedBase64)
}

fn encode_base64(value: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(value)
}

/// Strip PKCS#7 style padding, rejecting every malformed shape.
fn strip_pkcs7_padding(data: &[u8]) -> Result<&[u8], RstTransportError> {
    let last = *data.last().ok_or(RstTransportError::InvalidPadding)?;
    let padding = usize::from(last);
    if padding == 0 || padding > AES_BLOCK_SIZE || padding > data.len() {
        return Err(RstTransportError::InvalidPadding);
    }
    let plain_len = data.len() - padding;
    let tail = data.get(plain_len..).ok_or(RstTransportError::InvalidPadding)?;
    if tail.iter().any(|byte| usize::from(*byte) != padding) {
        return Err(RstTransportError::InvalidPadding);
    }
    data.get(..plain_len)
        .ok_or(RstTransportError::InvalidPadding)
}

/// Decrypt one AES-CBC payload.
///
/// The key is the 32-byte double-derived key, as in the reference client
/// (`aes.NewCipher` with a 32-byte key selects AES-256). An explicit
/// `EncryptionMethod` may narrow the key to AES-128 or AES-192; any other
/// declared algorithm is rejected rather than guessed.
fn aes_cbc_decrypt(
    derived_key: &[u8; 32],
    algorithm: Option<&str>,
    iv: &[u8],
    data: &[u8],
) -> Result<Vec<u8>, RstTransportError> {
    if iv.len() != AES_BLOCK_SIZE
        || data.is_empty()
        || !data.len().is_multiple_of(AES_BLOCK_SIZE)
    {
        return Err(RstTransportError::InvalidCiphertextLength);
    }
    let key: &[u8] = match algorithm {
        None => derived_key.as_slice(),
        Some(uri) if uri.ends_with("aes256-cbc") => derived_key.as_slice(),
        Some(uri) if uri.ends_with("aes128-cbc") => derived_key
            .get(..16)
            .ok_or(RstTransportError::Crypto)?,
        Some(uri) if uri.ends_with("aes192-cbc") => derived_key
            .get(..24)
            .ok_or(RstTransportError::Crypto)?,
        Some(_) => return Err(RstTransportError::UnsupportedEncryption),
    };
    if key.len() == AES_BLOCK_SIZE {
        // AES-128 path shared with the CLEP helpers.
        return cbc_decrypt(key, iv, data).map_err(|_| RstTransportError::Crypto);
    }
    let cipher = match key.len() {
        24 => Cipher::aes_192_cbc(),
        32 => Cipher::aes_256_cbc(),
        _ => return Err(RstTransportError::UnsupportedEncryption),
    };
    let mut crypter =
        Crypter::new(cipher, Mode::Decrypt, key, Some(iv)).map_err(|_| RstTransportError::Crypto)?;
    crypter.pad(false);
    let capacity = data
        .len()
        .checked_add(AES_BLOCK_SIZE)
        .ok_or(RstTransportError::Crypto)?;
    let mut output = vec![0u8; capacity];
    let written = crypter
        .update(data, &mut output)
        .map_err(|_| RstTransportError::Crypto)?;
    let tail = output
        .get_mut(written..)
        .ok_or(RstTransportError::Crypto)?;
    let finalized = crypter
        .finalize(tail)
        .map_err(|_| RstTransportError::Crypto)?;
    output.truncate(written.checked_add(finalized).ok_or(RstTransportError::Crypto)?);
    Ok(output)
}

// ---------------------------------------------------------------------------
// UTC formatting (no chrono dependency)
// ---------------------------------------------------------------------------

fn system_time_seconds(now: SystemTime) -> Result<u64, RstTransportError> {
    now.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| RstTransportError::InvalidClock)
}

/// Format a Unix timestamp as RFC 3339 in UTC, matching Go's `time.RFC3339`
/// rendering used by the reference client.
fn format_rfc3339_utc(unix_seconds: u64) -> Result<String, RstTransportError> {
    let days = i64::try_from(unix_seconds / 86_400).map_err(|_| RstTransportError::InvalidClock)?;
    let second_of_day = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days)?;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3_600,
        (second_of_day % 3_600) / 60,
        second_of_day % 60
    ))
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> Result<(i64, u64, u64), RstTransportError> {
    let shifted = days
        .checked_add(719_468)
        .ok_or(RstTransportError::InvalidClock)?;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted
            .checked_sub(146_096)
            .ok_or(RstTransportError::InvalidClock)?
    } / 146_097;
    let day_of_era =
        u64::try_from(shifted - era * 146_097).map_err(|_| RstTransportError::InvalidClock)?;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = i64::try_from(year_of_era)
        .map_err(|_| RstTransportError::InvalidClock)?
        .checked_add(era.checked_mul(400).ok_or(RstTransportError::InvalidClock)?)
        .ok_or(RstTransportError::InvalidClock)?;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 {
        year.checked_add(1).ok_or(RstTransportError::InvalidClock)?
    } else {
        year
    };
    Ok((year, month, day))
}

// ---------------------------------------------------------------------------
// RST request construction
// ---------------------------------------------------------------------------

/// Signature algorithm used by an [`RstDocument`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RstSigning {
    /// RSA PKCS#1 v1.5 over SHA-256 (device request, no derived key).
    RsaSha256,
    /// HMAC-SHA256 over the WS-SecureConversation double-derived key.
    HmacSha256,
}

/// A built RST envelope, ready to be canonicalized and POSTed.
///
/// `Debug` is implemented by hand: the tree carries the caller's ticket.
pub struct RstDocument {
    document: XmlDocument,
    nonce: Vec<u8>,
    signing: RstSigning,
}

impl fmt::Debug for RstDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RstDocument")
            .field("signing", &self.signing)
            .field("nonce_length", &self.nonce.len())
            .finish_non_exhaustive()
    }
}

impl RstDocument {
    /// Parsed envelope tree.
    pub fn document(&self) -> &XmlDocument {
        &self.document
    }

    /// Root element of the envelope.
    pub fn root(&self) -> XmlNode {
        self.document.root()
    }

    /// Signature algorithm used for `SignedInfo`.
    pub fn signing(&self) -> RstSigning {
        self.signing
    }

    /// The 32-byte client nonce of this request. It is public request material
    /// and is carried inside the `wssc:DerivedKeyToken` for SSO (HMAC) requests;
    /// RSA-signed device requests do not embed it.
    pub fn nonce(&self) -> &[u8] {
        &self.nonce
    }

    /// Exclusive canonical form of the envelope; this is the POST body.
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, RstTransportError> {
        self.document.canonicalize_root()
    }
}

/// Build an RST request, drawing a fresh 32-byte nonce and using the wall clock.
///
/// `ticket` is the SAML assertion obtained from a previous exchange: when it is
/// present the request is SSO-style (ticket, `wssc:DerivedKeyToken`, HMAC-SHA256
/// over `derived(secret, nonce)`); when it is absent the request is device-style
/// (`wsse:UsernameToken`, RSA PKCS#1 v1.5 over SHA-256 with `rsa_key`).
pub fn make_rst(
    member: &str,
    scope: &str,
    ticket: Option<&str>,
    rsa_key: Option<&PKey<Private>>,
    secret: Option<&[u8]>,
) -> Result<RstDocument, RstTransportError> {
    let mut nonce = vec![0u8; RST_NONCE_LENGTH];
    rand_bytes(&mut nonce).map_err(|_| RstTransportError::Crypto)?;
    make_rst_with(
        member,
        scope,
        ticket,
        rsa_key,
        secret,
        SystemTime::now(),
        &nonce,
    )
}

/// Deterministic core of [`make_rst`], with an explicit clock and nonce.
///
/// The nonce is public request material (it travels base64-encoded inside the
/// envelope) and is injected here so callers can reproduce an exchange and tests
/// stay offline and repeatable.
pub fn make_rst_with(
    member: &str,
    scope: &str,
    ticket: Option<&str>,
    rsa_key: Option<&PKey<Private>>,
    secret: Option<&[u8]>,
    now: SystemTime,
    nonce: &[u8],
) -> Result<RstDocument, RstTransportError> {
    let ticket = ticket.filter(|value| !value.trim().is_empty());
    if scope.trim().is_empty() {
        return Err(RstTransportError::EmptyScope);
    }
    let signing = match (rsa_key.is_some(), ticket.is_some()) {
        (true, _) => RstSigning::RsaSha256,
        (false, true) => RstSigning::HmacSha256,
        (false, false) => return Err(RstTransportError::MissingSigningKey),
    };
    let secret = secret.filter(|value| !value.is_empty());
    if signing == RstSigning::HmacSha256 {
        if nonce.len() != RST_NONCE_LENGTH {
            return Err(RstTransportError::InvalidNonce);
        }
        if secret.is_none() {
            return Err(RstTransportError::EmptySecret);
        }
    }
    let created = system_time_seconds(now)?;
    let expires = created
        .checked_add(RST_TIMESTAMP_LIFETIME)
        .ok_or(RstTransportError::InvalidClock)?;

    let mut document = XmlDocument::new();
    let envelope = document.create_element("s:Envelope")?;
    for (prefix, uri) in ENVELOPE_NAMESPACES {
        document.declare_namespace(envelope, prefix, uri)?;
    }

    let header = document.append_element(envelope, "s:Header")?;
    let action = document.append_text_element(header, "wsa:Action", RST_ACTION)?;
    document.set_attribute(action, "s:mustUnderstand", "1")?;
    let to = document.append_text_element(header, "wsa:To", RST_TO)?;
    document.set_attribute(to, "s:mustUnderstand", "1")?;
    document.append_text_element(header, "wsa:MessageID", &created.to_string())?;

    let auth_info = document.append_element(header, "ps:AuthInfo")?;
    document.set_attribute(auth_info, "Id", "PPAuthInfo")?;
    let sso = ticket.is_some();
    document.append_text_element(
        auth_info,
        "ps:SSOFlags",
        if sso { SSO_FLAGS } else { "" },
    )?;
    document.append_text_element(
        auth_info,
        "ps:HostingApp",
        if sso {
            HOSTING_APP_SSO
        } else {
            HOSTING_APP_DEVICE
        },
    )?;
    for (name, value) in AUTH_INFO_FIELDS {
        document.append_text_element(auth_info, &format!("ps:{name}"), value)?;
    }

    let security = document.append_element(header, "wsse:Security")?;
    match ticket {
        Some(ticket) => {
            // The ticket subtree is inserted verbatim: it is caller-supplied
            // material, never re-signed or rewritten here.
            let ticket_document = XmlDocument::parse(ticket.as_bytes())?;
            document.append_subtree(security, &ticket_document, ticket_document.root())?;
            let derived_token = document.append_element(security, "wssc:DerivedKeyToken")?;
            document.set_attribute(derived_token, "wsu:Id", "SignKey")?;
            document.set_attribute(derived_token, "Algorithm", DERIVED_KEY_ALGORITHM)?;
            let requested_reference =
                document.append_element(derived_token, "RequestedTokenReference")?;
            let identifier =
                document.append_text_element(requested_reference, "wsse:KeyIdentifier", "")?;
            document.set_attribute(identifier, "ValueType", KEY_IDENTIFIER_VALUE_TYPE)?;
            let reference = document.append_element(requested_reference, "wsse:Reference")?;
            document.set_attribute(reference, "URI", "")?;
            document.append_text_element(derived_token, "wssc:Nonce", &encode_base64(nonce))?;
        }
        None => {
            let username_token = document.append_element(security, "wsse:UsernameToken")?;
            document.set_attribute(username_token, "wsu:Id", "devicesoftware")?;
            document.append_text_element(username_token, "wsse:Username", member)?;
        }
    }

    let timestamp = document.append_element(security, "wsu:Timestamp")?;
    document.set_attribute(timestamp, "wsu:Id", "Timestamp")?;
    document.append_text_element(timestamp, "wsu:Created", &format_rfc3339_utc(created)?)?;
    document.append_text_element(timestamp, "wsu:Expires", &format_rfc3339_utc(expires)?)?;

    let body = document.append_element(envelope, "s:Body")?;
    let request = document.append_element(body, "wst:RequestSecurityToken")?;
    document.set_attribute(request, "Id", "RST0")?;
    document.append_text_element(request, "wst:RequestType", RST_REQUEST_TYPE)?;
    let applies_to = document.append_element(request, "wsp:AppliesTo")?;
    let endpoint = document.append_element(applies_to, "wsa:EndpointReference")?;
    document.append_text_element(endpoint, "wsa:Address", scope)?;
    if scope != TOKEN_BROKER_SCOPE {
        let policy = document.append_element(request, "wsp:PolicyReference")?;
        document.set_attribute(policy, "URI", "MBI_SSL")?;
    }

    // The signature is appended after the signed targets exist, exactly as the
    // reference client does: a reference digest never covers the signature.
    let signature = document.append_element(security, "Signature")?;
    document.declare_namespace(signature, "", DSIG_NAMESPACE)?;
    let signed_info = document.append_element(signature, "SignedInfo")?;
    let canonicalization_method =
        document.append_element(signed_info, "CanonicalizationMethod")?;
    document.set_attribute(
        canonicalization_method,
        "Algorithm",
        EXCLUSIVE_C14N_NAMESPACE,
    )?;
    let signature_method = document.append_element(signed_info, "SignatureMethod")?;
    document.set_attribute(
        signature_method,
        "Algorithm",
        match signing {
            RstSigning::RsaSha256 => SIGNATURE_METHOD_RSA_SHA256,
            RstSigning::HmacSha256 => SIGNATURE_METHOD_HMAC_SHA256,
        },
    )?;
    for target in [request, timestamp, auth_info] {
        let reference = document.append_element(signed_info, "Reference")?;
        let id = document
            .element_id(target)
            .ok_or(RstTransportError::ElementCount("Id", 0))?;
        document.set_attribute(reference, "URI", &format!("#{id}"))?;
        let transforms = document.append_element(reference, "Transforms")?;
        let transform = document.append_element(transforms, "Transform")?;
        document.set_attribute(transform, "Algorithm", EXCLUSIVE_C14N_NAMESPACE)?;
        let digest_method = document.append_element(reference, "DigestMethod")?;
        document.set_attribute(digest_method, "Algorithm", DIGEST_METHOD_SHA256)?;
        let digest = Sha256::digest(document.canonicalize(target)?);
        document.append_text_element(reference, "DigestValue", &encode_base64(&digest))?;
    }
    let canonical_signed_info = document.canonicalize(signed_info)?;
    let signature_value = match (signing, rsa_key, secret) {
        (RstSigning::RsaSha256, Some(key), _) => rsa_sign(key, &canonical_signed_info)?,
        (RstSigning::HmacSha256, _, Some(secret)) => {
            let key = derived(secret, nonce)?;
            hmac_sha256(&key, &canonical_signed_info)?
        }
        _ => return Err(RstTransportError::MissingSigningKey),
    };
    document.append_text_element(signature, "SignatureValue", &encode_base64(&signature_value))?;
    if ticket.is_some() {
        let key_info = document.append_element(signature, "KeyInfo")?;
        let token_reference =
            document.append_element(key_info, "wsse:SecurityTokenReference")?;
        let reference = document.append_element(token_reference, "wsse:Reference")?;
        document.set_attribute(reference, "URI", "#SignKey")?;
    }

    Ok(RstDocument {
        document,
        nonce: nonce.to_vec(),
        signing,
    })
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// Transport context for one RST exchange.
///
/// [`Default`] is the exact endpoint, headers and response ceiling the reference
/// client uses. The caller-supplied [`Client`] is expected not to follow
/// redirects (`reqwest`'s policy is per client) and to carry the proxy policy
/// from [`http_client`](crate::services::http_client).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RstTransportContext {
    /// Absolute URL the envelope is POSTed to.
    pub endpoint: String,
    /// `User-Agent` header.
    pub user_agent: String,
    /// `Content-Type` header.
    pub content_type: String,
    /// Hard response body ceiling in bytes.
    pub max_response_bytes: usize,
}

impl Default for RstTransportContext {
    fn default() -> Self {
        Self {
            endpoint: RST_ENDPOINT.to_string(),
            user_agent: RST_USER_AGENT.to_string(),
            content_type: RST_CONTENT_TYPE.to_string(),
            max_response_bytes: RST_RESPONSE_LIMIT,
        }
    }
}

/// Outcome of validating an encrypted response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RstDecryption {
    /// The response signature and every reference digest were verified.
    pub signature_verified: bool,
    /// Number of `EncryptedData` payloads that were decrypted in place.
    pub payloads_decrypted: usize,
}

/// A parsed RST response. `Debug` is implemented by hand because the body can
/// contain tokens.
pub struct RstResponse {
    http_status: u16,
    body_bytes: usize,
    document: XmlDocument,
    decryption: Option<RstDecryption>,
}

impl fmt::Debug for RstResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RstResponse")
            .field("http_status", &self.http_status)
            .field("body_bytes", &self.body_bytes)
            .field("decryption", &self.decryption)
            .finish_non_exhaustive()
    }
}

impl RstResponse {
    /// HTTP status of the exchange.
    pub fn http_status(&self) -> u16 {
        self.http_status
    }

    /// Size of the response body that was parsed.
    pub fn body_bytes(&self) -> usize {
        self.body_bytes
    }

    /// Whether the endpoint answered with a 2xx status.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.http_status)
    }

    /// Parsed response tree.
    pub fn document(&self) -> &XmlDocument {
        &self.document
    }

    /// Root element of the response.
    pub fn root(&self) -> XmlNode {
        self.document.root()
    }

    /// Number of SOAP `Fault` elements in the response.
    pub fn fault_count(&self) -> usize {
        self.document.find_all(self.root(), "Fault").len()
    }

    /// Number of `RequestedSecurityToken` elements in the response.
    pub fn requested_security_token_count(&self) -> usize {
        self.document
            .find_all(self.root(), "RequestedSecurityToken")
            .len()
    }

    /// Verification result, absent when no secret was supplied.
    pub fn decryption(&self) -> Option<RstDecryption> {
        self.decryption
    }

    /// The single element carried by the single `RequestedSecurityToken`.
    pub fn requested_token(&self) -> Result<XmlNode, RstTransportError> {
        let token = self
            .document
            .find_unique(self.root(), "RequestedSecurityToken")?;
        let children = self.document.children(token);
        match children.len() {
            1 => children
                .into_iter()
                .next()
                .ok_or(RstTransportError::ElementCount("RequestedSecurityToken child", 0)),
            count => Err(RstTransportError::ElementCount(
                "RequestedSecurityToken child",
                count,
            )),
        }
    }

    /// Serialized form of [`Self::requested_token`].
    pub fn requested_token_xml(&self) -> Result<String, RstTransportError> {
        let token = self.requested_token()?;
        self.document.to_xml(token)
    }

    /// Text of the single `BinarySecret` element.
    pub fn binary_secret(&self) -> Result<String, RstTransportError> {
        let secret = self.document.find_unique(self.root(), "BinarySecret")?;
        Ok(self.document.text(secret))
    }

    /// Text of the single `BinarySecurityToken` inside `RequestedSecurityToken`.
    pub fn binary_security_token(&self) -> Result<String, RstTransportError> {
        let token = self
            .document
            .find_unique(self.root(), "RequestedSecurityToken")?;
        let binary = self
            .document
            .find_unique(token, "BinarySecurityToken")?;
        Ok(self.document.text(binary))
    }
}

/// POST a canonicalized RST envelope and parse the bounded response.
///
/// When `secret` is supplied, the response must be signature-verified and every
/// `EncryptedData` payload decrypted before it is returned; an unsigned response
/// without a fault is rejected outright. A 2xx status is not required: a fault
/// body is parsed and returned so the caller can read the fault detail.
pub async fn post_rst(
    client: &Client,
    context: &RstTransportContext,
    document: &RstDocument,
    secret: Option<&[u8]>,
) -> Result<RstResponse, RstTransportError> {
    let body = document.to_canonical_bytes()?;
    let response = client
        .post(&context.endpoint)
        .header(reqwest::header::CONTENT_TYPE, context.content_type.as_str())
        .header(reqwest::header::USER_AGENT, context.user_agent.as_str())
        .body(body)
        .send()
        .await
        .map_err(|error| RstTransportError::Http(error.to_string()))?;
    let http_status = response.status().as_u16();
    let bytes = read_bounded_response(response, context.max_response_bytes).await?;
    let body_bytes = bytes.len();
    let mut parsed = XmlDocument::parse(&bytes)?;
    let root = parsed.root();
    let fault_count = parsed.find_all(root, "Fault").len();
    let requested_token_count = parsed.find_all(root, "RequestedSecurityToken").len();
    let mut decryption = None;
    if let Some(secret) = secret.filter(|value| !value.is_empty()) {
        let header_signatures = parsed
            .find_all(root, "Header")
            .first()
            .map_or(0, |header| parsed.find_all(*header, "Signature").len());
        if header_signatures == 1 {
            decryption = Some(decrypt_rst_response(&mut parsed, secret)?);
        } else if fault_count == 0 {
            return Err(RstTransportError::UnsignedSuccessResponse);
        }
    }
    log::debug!(
        target: "copper_core::services::store_rst_transport",
        "rst exchange status={http_status} bytes={body_bytes} faults={fault_count} requested_tokens={requested_token_count}"
    );
    Ok(RstResponse {
        http_status,
        body_bytes,
        document: parsed,
        decryption,
    })
}

async fn read_bounded_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, RstTransportError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(RstTransportError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| RstTransportError::Http(error.to_string()))?
    {
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(RstTransportError::ResponseTooLarge)?;
        if next_len > limit {
            return Err(RstTransportError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Response verification and decryption
// ---------------------------------------------------------------------------

/// Verify a response and decrypt its payloads in place.
///
/// The order matches the reference implementation and never trusts the document
/// structure before it is authenticated:
///
/// 1. collect `DerivedKeyToken` nonces;
/// 2. resolve the signature key from `Signature/KeyInfo/Reference` and verify
///    HMAC-SHA256 over the canonical `SignedInfo`;
/// 3. verify every `Reference/DigestValue` against the canonical signed target;
/// 4. decrypt each `EncryptedData` under `s:Body` or `ps:EncryptedPP`, checking
///    the padding, and splice the plaintext element back in place of the
///    ciphertext (in place of `ps:EncryptedPP` itself for header payloads).
pub fn decrypt_rst_response(
    document: &mut XmlDocument,
    secret: &[u8],
) -> Result<RstDecryption, RstTransportError> {
    if secret.is_empty() {
        return Err(RstTransportError::EmptySecret);
    }
    if document.elements.is_empty() {
        return Err(RstTransportError::MalformedXml);
    }
    let root = document.root();

    let mut nonces: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for derived_token in document.find_all(root, "DerivedKeyToken") {
        let id = document
            .element_id(derived_token)
            .ok_or(RstTransportError::MissingDerivedKeyId)?;
        let nonce = document.find_unique(derived_token, "Nonce")?;
        nonces.insert(id.to_string(), decode_base64(&document.text(nonce))?);
    }

    let header = document.find_unique(root, "Header")?;
    let signature = document.find_unique(header, "Signature")?;
    let key_info = document.find_unique(signature, "KeyInfo")?;
    let key_reference = document.find_unique(key_info, "Reference")?;
    let key_uri = document.attribute(key_reference, "URI").unwrap_or_default();
    let nonce = nonces
        .get(key_uri.strip_prefix('#').unwrap_or(key_uri))
        .ok_or(RstTransportError::MissingSignatureNonce)?;
    let signature_key = derived(secret, nonce)?;

    let signed_info = document.find_unique(signature, "SignedInfo")?;
    let canonical_signed_info = document.canonicalize(signed_info)?;
    let expected_signature = hmac_sha256(&signature_key, &canonical_signed_info)?;
    let signature_value_node = document.find_unique(signature, "SignatureValue")?;
    let signature_value = decode_base64(&document.text(signature_value_node))?;
    if !constant_time_eq(&expected_signature, &signature_value) {
        return Err(RstTransportError::SignatureMismatch);
    }

    for reference in document.children(signed_info) {
        if document.local_name(reference) != "Reference" {
            continue;
        }
        let uri = document.attribute(reference, "URI").unwrap_or_default();
        let id = uri.strip_prefix('#').unwrap_or(uri);
        let targets: Vec<XmlNode> = document
            .descendants(root)
            .into_iter()
            .filter(|candidate| document.element_id(*candidate) == Some(id))
            .collect();
        if targets.len() != 1 {
            return Err(RstTransportError::AmbiguousSignedTarget);
        }
        let target = targets
            .into_iter()
            .next()
            .ok_or(RstTransportError::AmbiguousSignedTarget)?;
        let digest = Sha256::digest(document.canonicalize(target)?);
        let digest_value_node = document.find_unique(reference, "DigestValue")?;
        let expected_digest = decode_base64(&document.text(digest_value_node))?;
        if !constant_time_eq(&digest, &expected_digest) {
            return Err(RstTransportError::SignedDigestMismatch);
        }
    }

    let encrypted_payloads = document.find_all(root, "EncryptedData");
    let mut payloads_decrypted = 0usize;
    for encrypted in encrypted_payloads {
        let Some(parent) = document.parent(encrypted) else {
            continue;
        };
        let parent_name = document.local_name(parent).to_string();
        if parent_name != "Body" && parent_name != "EncryptedPP" {
            continue;
        }
        let key_info = document.find_unique(encrypted, "KeyInfo")?;
        let key_reference = document.find_unique(key_info, "Reference")?;
        let key_uri = document.attribute(key_reference, "URI").unwrap_or_default();
        let nonce = nonces
            .get(key_uri.strip_prefix('#').unwrap_or(key_uri))
            .ok_or(RstTransportError::MissingEncryptionNonce)?;
        let payload_key = derived(secret, nonce)?;
        let cipher_value_node = document.find_unique(encrypted, "CipherValue")?;
        let cipher_text = decode_base64(&document.text(cipher_value_node))?;
        if cipher_text.len() < AES_BLOCK_SIZE.checked_mul(2).ok_or(RstTransportError::Crypto)? {
            return Err(RstTransportError::ShortCiphertext);
        }
        let (iv, cipher_body) = cipher_text.split_at(AES_BLOCK_SIZE);
        let algorithm = document
            .find_all(encrypted, "EncryptionMethod")
            .first()
            .and_then(|method| document.attribute(*method, "Algorithm"));
        let plain = aes_cbc_decrypt(&payload_key, algorithm, iv, cipher_body)?;
        let unpadded = strip_pkcs7_padding(&plain)?;
        let payload = XmlDocument::parse(unpadded)?;
        if parent_name == "Body" {
            document.replace_child(parent, encrypted, &payload, payload.root())?;
        } else {
            let header = document
                .parent(parent)
                .ok_or(RstTransportError::MalformedXml)?;
            document.replace_child(header, parent, &payload, payload.root())?;
        }
        payloads_decrypted = payloads_decrypted
            .checked_add(1)
            .ok_or(RstTransportError::MalformedXml)?;
    }

    Ok(RstDecryption {
        signature_verified: true,
        payloads_decrypted,
    })
}

// ---------------------------------------------------------------------------
// Device ticket acquisition: the two-stage RST exchange
// ---------------------------------------------------------------------------

/// SSO relying party of the second (ticket-bearing) exchange, as in the
/// reference `ownDeviceTicket`.
const DEVICE_SSO_SCOPE: &str = "www.microsoft.com";
/// Length of the CLEP-wrapped device proof the endpoint returns.
const DEVICE_PROOF_CLEP_LENGTH: usize = 48;
/// Offset of the 32-byte WS-SecureConversation secret inside the unwrapped proof.
const DEVICE_SECRET_RANGE: std::ops::Range<usize> = 12..44;

/// Unwrap the `BinarySecret` proof of the device exchange into the raw
/// WS-SecureConversation secret: `clep(base64_decode(proof), 48)[12..44]`.
///
/// The 4096-byte CLEP record is validated by [`decrypt_clep`]; a proof that is
/// not a well-formed record, or is too short to hold the secret, is rejected
/// rather than padded or guessed.
fn device_proof_secret(proof: &[u8]) -> Result<[u8; 32], StoreRstError> {
    let unwrapped = decrypt_clep(proof, DEVICE_PROOF_CLEP_LENGTH)?;
    let secret = unwrapped.get(DEVICE_SECRET_RANGE.clone()).ok_or_else(|| {
        StoreRstError::Exchange("unwrapped device proof is too short".to_string())
    })?;
    let mut result = [0u8; 32];
    result.copy_from_slice(secret);
    Ok(result)
}

/// Acquire a Store device ticket through the two-stage RST exchange.
///
/// Stage 1 authenticates the device software credential: an RST signed with the
/// caller's device RSA key (`member` comes from the DPAPI-protected device
/// cache) returns the device SAML assertion plus a CLEP-wrapped `BinarySecret`
/// proof. Stage 2 unwraps that proof into the WS-SecureConversation secret, and
/// stage 3 issues the Store ticket with an HMAC-signed, encrypted RST that is
/// verified and decrypted before its token is read.
///
/// The private key and member name are caller-supplied: nothing here invents a
/// credential, and the returned ticket is exactly what the endpoint issued.
#[cfg(windows)]
pub async fn acquire_device_ticket(
    client: &Client,
    member: &str,
    private_key: &Rsa<Private>,
) -> Result<String, StoreRstError> {
    if member.trim().is_empty() {
        return Err(StoreRstError::Exchange(
            "device member name is empty".to_string(),
        ));
    }
    let context = RstTransportContext::default();

    // Stage 1: device RST signed with RSA-PKCS1v15(SHA-256); no shared secret yet.
    let device_key = PKey::from_rsa(private_key.clone()).map_err(|_| StoreRstError::Crypto)?;
    let device_request = make_rst(member, TOKEN_BROKER_SCOPE, None, Some(&device_key), None)?;
    let device_response = post_rst(client, &context, &device_request, None).await?;
    let ticket = device_response.requested_token_xml()?;

    // Stage 2: unwrap the CLEP proof into the derived-key secret.
    let proof = decode_base64(&device_response.binary_secret()?)?;
    let mut secret = device_proof_secret(&proof)?;

    // Stage 3: SSO RST signed with HMAC-SHA256 over the derived key; the
    // response is verified and decrypted before its ticket is read.
    let sso_request = make_rst(
        "",
        DEVICE_SSO_SCOPE,
        Some(&ticket),
        None,
        Some(&secret),
    )?;
    let sso_response = post_rst(client, &context, &sso_request, Some(&secret)).await;
    let token = sso_response?.binary_security_token()?;
    secret.fill(0);
    Ok(token)
}

/// Non-Windows builds have no device credential store: the exchange is refused
/// instead of being simulated. The transport primitives themselves stay
/// platform-agnostic; only this entry point mirrors the `rst_windows.go` build
/// gate of the reference client.
#[cfg(not(windows))]
pub async fn acquire_device_ticket(
    _client: &Client,
    _member: &str,
    _private_key: &Rsa<Private>,
) -> Result<String, StoreRstError> {
    Err(StoreRstError::WindowsOnly)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::rsa::Rsa;
    use openssl::sign::Verifier;
    use std::time::Duration;

    const TEST_SECRET: [u8; 32] = [42u8; 32];
    const TEST_NONCE: [u8; 32] = [17u8; 32];
    const TEST_TIME: u64 = 1_700_000_000;

    fn test_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(TEST_TIME)
    }

    fn parse(xml: &[u8]) -> XmlDocument {
        XmlDocument::parse(xml).expect("document parses")
    }

    fn canonical_text(document: &XmlDocument, node: XmlNode) -> String {
        String::from_utf8(document.canonicalize(node).expect("canonicalizes"))
            .expect("canonical output is UTF-8")
    }

    /// Reference vector for `derived` computed independently with .NET
    /// `HMACSHA256` over `00000001 || "WS-SecureConversation" x2 || 00 || nonce || 00000100`.
    const DERIVED_VECTOR: &str = "916d59f30355cafb681fd9bf4c393abed4a2a1c05d4f3d0b7a065dde63c8b6cd";

    #[test]
    fn derived_key_is_reproducible_and_matches_the_reference_vector() {
        let key = derived(&TEST_SECRET, &TEST_NONCE).expect("derives");
        assert_eq!(key, derived(&TEST_SECRET, &TEST_NONCE).expect("derives"));
        assert_eq!(key.to_vec(), crate::services::store_rst::derived_key(&TEST_SECRET, &TEST_NONCE).expect("derives").to_vec());
        let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, DERIVED_VECTOR);
        assert_eq!(derived(&[], &TEST_NONCE), Err(RstTransportError::Crypto));
    }

    #[test]
    fn canonicalization_escapes_text_and_attribute_values() {
        let document = parse(br#"<n:r xmlns:n="urn:n" x="&lt;&amp;&quot;"><n:c>a&amp;b&lt;c&gt;d</n:c></n:r>"#);
        assert_eq!(
            canonical_text(&document, document.root()),
            r#"<n:r xmlns:n="urn:n" x="&lt;&amp;&quot;"><n:c>a&amp;b&lt;c&gt;d</n:c></n:r>"#
        );

        let mut built = XmlDocument::new();
        let root = built.create_element("r").expect("creates");
        built
            .set_attribute(root, "x", "q\"uote\ttab\nline")
            .expect("sets");
        assert_eq!(
            canonical_text(&built, root),
            "<r x=\"q&quot;uote&#x9;tab&#xA;line\"></r>"
        );
    }

    #[test]
    fn canonicalization_sorts_attributes_by_namespace_then_local_name() {
        let document = parse(
            br#"<r xmlns:b="urn:b" z="1" a="2" b:m="3" b:a="4"/>"#,
        );
        assert_eq!(
            canonical_text(&document, document.root()),
            r#"<r xmlns:b="urn:b" a="2" z="1" b:a="4" b:m="3"></r>"#
        );
    }

    #[test]
    fn canonicalization_drops_unused_namespaces_and_keeps_the_default_one_first() {
        let document = parse(br#"<e xmlns:z="urn:z" xmlns="urn:d" xmlns:a="urn:a"><a:x/></e>"#);
        assert_eq!(
            canonical_text(&document, document.root()),
            r#"<e xmlns="urn:d"><a:x xmlns:a="urn:a"></a:x></e>"#
        );
    }

    #[test]
    fn canonicalization_of_a_subtree_sees_inherited_namespaces() {
        // `SignedInfo` is unprefixed and relies on `Signature`'s default
        // namespace, the exact case that makes a detached digest work.
        let document = parse(
            br##"<s:Envelope xmlns:s="urn:s"><s:Header><Signature xmlns="http://www.w3.org/2000/09/xmldsig#"><SignedInfo><Reference URI="#x"></Reference></SignedInfo></Signature></s:Header></s:Envelope>"##,
        );
        let signed_info = document
            .find_unique(document.root(), "SignedInfo")
            .expect("unique SignedInfo");
        assert_eq!(
            canonical_text(&document, signed_info),
            r##"<SignedInfo xmlns="http://www.w3.org/2000/09/xmldsig#"><Reference URI="#x"></Reference></SignedInfo>"##
        );
    }

    #[test]
    fn padding_validation_rejects_bad_input() {
        assert_eq!(strip_pkcs7_padding(&[]), Err(RstTransportError::InvalidPadding));
        assert_eq!(
            strip_pkcs7_padding(&[1, 2, 3, 0]),
            Err(RstTransportError::InvalidPadding)
        );
        assert_eq!(
            strip_pkcs7_padding(&[1, 2, 3, 17]),
            Err(RstTransportError::InvalidPadding)
        );
        assert_eq!(
            strip_pkcs7_padding(&[1, 2, 3, 2]),
            Err(RstTransportError::InvalidPadding)
        );
        assert_eq!(strip_pkcs7_padding(&[1, 2, 3, 4, 4, 4, 4]).expect("strips"), &[1, 2, 3]);
        assert_eq!(strip_pkcs7_padding(&[7u8; 16]).expect("strips"), &[7u8; 9]);
        // A complete padding block is accepted when every byte agrees.
        let mut full_block = vec![0u8; AES_BLOCK_SIZE];
        full_block.extend_from_slice(&[16u8; AES_BLOCK_SIZE]);
        assert_eq!(strip_pkcs7_padding(&full_block).expect("strips"), &[0u8; 16]);
    }

    #[test]
    fn aes128_payloads_use_the_shared_clep_helper() {
        let mut derived_key = [0u8; 32];
        derived_key[..AES_BLOCK_SIZE].copy_from_slice(&[9u8; AES_BLOCK_SIZE]);
        let iv = [1u8; AES_BLOCK_SIZE];
        let mut plain = b"store rst".to_vec();
        let padding = AES_BLOCK_SIZE - (plain.len() % AES_BLOCK_SIZE);
        plain.extend(std::iter::repeat_n(padding as u8, padding));
        let mut crypter = Crypter::new(
            Cipher::aes_128_cbc(),
            Mode::Encrypt,
            &derived_key[..AES_BLOCK_SIZE],
            Some(&iv),
        )
        .expect("crypter");
        crypter.pad(false);
        let mut cipher_text = vec![0u8; plain.len() + AES_BLOCK_SIZE];
        let written = crypter.update(&plain, &mut cipher_text).expect("encrypts");
        let finalized = crypter
            .finalize(&mut cipher_text[written..])
            .expect("finalizes");
        cipher_text.truncate(written + finalized);
        let decrypted = aes_cbc_decrypt(
            &derived_key,
            Some("http://www.w3.org/2001/04/xmlenc#aes128-cbc"),
            &iv,
            &cipher_text,
        )
        .expect("decrypts");
        assert_eq!(
            strip_pkcs7_padding(&decrypted).expect("strips"),
            b"store rst"
        );
        // A short body, a bad length and an unknown algorithm are all refused.
        assert_eq!(
            aes_cbc_decrypt(&derived_key, None, &iv, &[0u8; 8]),
            Err(RstTransportError::InvalidCiphertextLength)
        );
        assert_eq!(
            aes_cbc_decrypt(&derived_key, Some("urn:unknown"), &iv, &cipher_text),
            Err(RstTransportError::UnsupportedEncryption)
        );
    }

    #[test]
    fn rfc3339_formatting_matches_utc_calendar() {
        assert_eq!(format_rfc3339_utc(0).expect("formats"), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_rfc3339_utc(1_700_000_000).expect("formats"),
            "2023-11-14T22:13:20Z"
        );
        assert_eq!(
            format_rfc3339_utc(1_709_164_800).expect("formats"),
            "2024-02-29T00:00:00Z"
        );
        assert_eq!(
            format_rfc3339_utc(2_147_483_647).expect("formats"),
            "2038-01-19T03:14:07Z"
        );
        assert_eq!(
            format_rfc3339_utc(4_102_444_800).expect("formats"),
            "2100-01-01T00:00:00Z"
        );
    }

    #[test]
    fn rst_request_with_a_ticket_is_self_consistent() {
        let ticket = r#"<saml:Assertion xmlns:saml="urn:oasis:names:tc:SAML:1.0:assertion" AssertionID="_t"><saml:marker>x</saml:marker></saml:Assertion>"#;
        let document = make_rst_with(
            "",
            "www.microsoft.com",
            Some(ticket),
            None,
            Some(&TEST_SECRET),
            test_time(),
            &TEST_NONCE,
        )
        .expect("builds");
        assert_eq!(document.signing(), RstSigning::HmacSha256);
        assert_eq!(document.nonce(), &TEST_NONCE);

        let tree = document.document();
        let root = document.root();
        let action = tree.find_unique(root, "Action").expect("unique Action");
        assert_eq!(tree.text(action), RST_ACTION);
        assert_eq!(tree.attribute(action, "s:mustUnderstand"), Some("1"));
        let to = tree.find_unique(root, "To").expect("unique To");
        assert_eq!(tree.text(to), RST_TO);
        let message_id = tree.find_unique(root, "MessageID").expect("unique MessageID");
        assert_eq!(tree.text(message_id), TEST_TIME.to_string());
        let created = tree.find_unique(root, "Created").expect("unique Created");
        assert_eq!(tree.text(created), "2023-11-14T22:13:20Z");
        let expires = tree.find_unique(root, "Expires").expect("unique Expires");
        assert_eq!(tree.text(expires), "2023-11-14T22:18:20Z");
        let sso_flags = tree.find_unique(root, "SSOFlags").expect("unique SSOFlags");
        assert_eq!(tree.text(sso_flags), SSO_FLAGS);
        let hosting_app = tree.find_unique(root, "HostingApp").expect("unique HostingApp");
        assert_eq!(tree.text(hosting_app), HOSTING_APP_SSO);
        assert_eq!(
            tree.attribute(
                tree.find_unique(root, "AuthInfo").expect("unique AuthInfo"),
                "Id"
            ),
            Some("PPAuthInfo")
        );
        let address = tree.find_unique(root, "Address").expect("unique Address");
        assert_eq!(tree.text(address), "www.microsoft.com");
        let policy = tree.find_unique(root, "PolicyReference").expect("unique policy");
        assert_eq!(tree.attribute(policy, "URI"), Some("MBI_SSL"));
        let derived_token = tree
            .find_unique(root, "DerivedKeyToken")
            .expect("unique DerivedKeyToken");
        assert_eq!(tree.element_id(derived_token), Some("SignKey"));
        assert_eq!(
            tree.attribute(derived_token, "Algorithm"),
            Some(DERIVED_KEY_ALGORITHM)
        );
        let nonce = tree.find_unique(derived_token, "Nonce").expect("unique Nonce");
        assert_eq!(decode_base64(&tree.text(nonce)).expect("decodes"), TEST_NONCE.to_vec());
        assert!(tree.find_all(root, "UsernameToken").is_empty());

        // Signature verification, using the same canonical form the peer signs.
        let signature = tree.find_unique(root, "Signature").expect("unique Signature");
        let signature_method = tree
            .find_unique(signature, "SignatureMethod")
            .expect("unique SignatureMethod");
        assert_eq!(
            tree.attribute(signature_method, "Algorithm"),
            Some(SIGNATURE_METHOD_HMAC_SHA256)
        );
        let canonicalization_method = tree
            .find_unique(signature, "CanonicalizationMethod")
            .expect("unique CanonicalizationMethod");
        assert_eq!(
            tree.attribute(canonicalization_method, "Algorithm"),
            Some(EXCLUSIVE_C14N_NAMESPACE)
        );
        let signed_info = tree.find_unique(signature, "SignedInfo").expect("unique SignedInfo");
        let key = derived(&TEST_SECRET, &TEST_NONCE).expect("derives");
        let mac = hmac_sha256(&key, &tree.canonicalize(signed_info).expect("canonicalizes"))
            .expect("macs");
        let signature_value = tree
            .find_unique(signature, "SignatureValue")
            .expect("unique SignatureValue");
        assert_eq!(tree.text(signature_value), encode_base64(&mac));

        // Every reference must cover exactly one existing target and match it.
        let references = tree.find_all(signed_info, "Reference");
        assert_eq!(references.len(), 3);
        let mut uris: Vec<String> = Vec::new();
        for reference in references {
            let uri = tree.attribute(reference, "URI").expect("URI attribute").to_string();
            uris.push(uri.clone());
            let id = uri.strip_prefix('#').expect("fragment reference");
            let targets: Vec<XmlNode> = tree
                .descendants(root)
                .into_iter()
                .filter(|candidate| tree.element_id(*candidate) == Some(id))
                .collect();
            assert_eq!(targets.len(), 1, "target {uri} must be unique");
            let digest_method = tree
                .find_unique(reference, "DigestMethod")
                .expect("unique DigestMethod");
            assert_eq!(
                tree.attribute(digest_method, "Algorithm"),
                Some(DIGEST_METHOD_SHA256)
            );
            let digest = Sha256::digest(tree.canonicalize(targets[0]).expect("canonicalizes"));
            let digest_value = tree
                .find_unique(reference, "DigestValue")
                .expect("unique DigestValue");
            assert_eq!(tree.text(digest_value), encode_base64(&digest));
        }
        assert_eq!(uris, ["#RST0", "#Timestamp", "#PPAuthInfo"].map(String::from).to_vec());

        // The POST body is the canonical envelope and stays parseable.
        let body = document.to_canonical_bytes().expect("canonical envelope");
        let reparsed = parse(&body);
        assert_eq!(reparsed.local_name(reparsed.root()), "Envelope");
        assert_eq!(
            format!("{:?}", document),
            "RstDocument { signing: HmacSha256, nonce_length: 32, .. }"
        );
    }

    #[test]
    fn rst_request_without_a_ticket_uses_rsa_and_the_device_username() {
        let rsa = Rsa::generate(2048).expect("generates key");
        let key = PKey::from_rsa(rsa).expect("wraps key");
        let document = make_rst_with(
            "02000000000000",
            TOKEN_BROKER_SCOPE,
            None,
            Some(&key),
            None,
            test_time(),
            &TEST_NONCE,
        )
        .expect("builds");
        assert_eq!(document.signing(), RstSigning::RsaSha256);

        let tree = document.document();
        let root = document.root();
        let username = tree.find_unique(root, "Username").expect("unique Username");
        assert_eq!(tree.text(username), "02000000000000");
        let username_token = tree
            .find_unique(root, "UsernameToken")
            .expect("unique UsernameToken");
        assert_eq!(tree.element_id(username_token), Some("devicesoftware"));
        assert!(tree.find_all(root, "DerivedKeyToken").is_empty());
        assert!(tree.find_all(root, "KeyInfo").is_empty());
        // The token-broker scope omits the policy reference.
        assert!(tree.find_all(root, "PolicyReference").is_empty());
        let hosting_app = tree.find_unique(root, "HostingApp").expect("unique HostingApp");
        assert_eq!(tree.text(hosting_app), HOSTING_APP_DEVICE);

        let signature = tree.find_unique(root, "Signature").expect("unique Signature");
        let signature_method = tree
            .find_unique(signature, "SignatureMethod")
            .expect("unique SignatureMethod");
        assert_eq!(
            tree.attribute(signature_method, "Algorithm"),
            Some(SIGNATURE_METHOD_RSA_SHA256)
        );
        let signed_info = tree.find_unique(signature, "SignedInfo").expect("unique SignedInfo");
        let signature_value = tree
            .find_unique(signature, "SignatureValue")
            .expect("unique SignatureValue");
        let mut verifier = Verifier::new(MessageDigest::sha256(), &key).expect("verifier");
        verifier
            .update(&tree.canonicalize(signed_info).expect("canonicalizes"))
            .expect("updates");
        assert!(verifier
            .verify(&decode_base64(&tree.text(signature_value)).expect("decodes"))
            .expect("verifies"));
    }

    /// Encrypted response fixture mirroring the reference test: an encrypted
    /// `ps:EncryptedPP` in the header, an encrypted body, and a signature over
    /// both under a derived key.
    fn encrypted_response_fixture() -> (XmlDocument, Vec<u8>, [u8; 32]) {
        let key = derived(&TEST_SECRET, &TEST_NONCE).expect("derives");
        let mut document = XmlDocument::new();
        let root = document.create_element("s:Envelope").expect("creates");
        for (prefix, uri) in ENVELOPE_NAMESPACES {
            document.declare_namespace(root, prefix, uri).expect("declares");
        }
        document
            .declare_namespace(root, "xenc", XMLENC_NAMESPACE)
            .expect("declares");
        let header = document.append_element(root, "s:Header").expect("appends");
        let security = document
            .append_element(header, "wsse:Security")
            .expect("appends");
        let derived_token = document
            .append_element(security, "wssc:DerivedKeyToken")
            .expect("appends");
        document
            .set_attribute(derived_token, "wsu:Id", "response-key")
            .expect("sets");
        document
            .append_text_element(
                derived_token,
                "wssc:Nonce",
                &encode_base64(&TEST_NONCE),
            )
            .expect("appends");
        let encrypted_pp = document
            .append_element(header, "ps:EncryptedPP")
            .expect("appends");
        document
            .set_attribute(encrypted_pp, "wsu:Id", "encrypted-pp")
            .expect("sets");
        let body = document.append_element(root, "s:Body").expect("appends");
        document
            .set_attribute(body, "wsu:Id", "response-body")
            .expect("sets");
        encrypt_payload(
            &mut document,
            encrypted_pp,
            &key,
            &format!(
                r#"<ps:AuthInfo xmlns:ps="{NS_PASSPORT}"><ps:marker>header</ps:marker></ps:AuthInfo>"#
            ),
        );
        encrypt_payload(
            &mut document,
            body,
            &key,
            &format!(
                r#"<wst:RequestSecurityTokenResponse xmlns:wst="{NS_WST}"><wst:marker>body</wst:marker></wst:RequestSecurityTokenResponse>"#
            ),
        );

        let signature = document
            .append_element(security, "Signature")
            .expect("appends");
        document
            .declare_namespace(signature, "", DSIG_NAMESPACE)
            .expect("declares");
        let signed_info = document
            .append_element(signature, "SignedInfo")
            .expect("appends");
        for target in [encrypted_pp, body] {
            let reference = document
                .append_element(signed_info, "Reference")
                .expect("appends");
            let id = document.element_id(target).expect("id").to_string();
            document
                .set_attribute(reference, "URI", &format!("#{id}"))
                .expect("sets");
            let digest =
                Sha256::digest(document.canonicalize(target).expect("canonicalizes"));
            document
                .append_text_element(reference, "DigestValue", &encode_base64(&digest))
                .expect("appends");
        }
        let mac = hmac_sha256(
            &key,
            &document.canonicalize(signed_info).expect("canonicalizes"),
        )
        .expect("macs");
        document
            .append_text_element(signature, "SignatureValue", &encode_base64(&mac))
            .expect("appends");
        let key_info = document
            .append_element(signature, "KeyInfo")
            .expect("appends");
        let token_reference = document
            .append_element(key_info, "wsse:SecurityTokenReference")
            .expect("appends");
        let reference = document
            .append_element(token_reference, "wsse:Reference")
            .expect("appends");
        document
            .set_attribute(reference, "URI", "#response-key")
            .expect("sets");
        (document, TEST_SECRET.to_vec(), key)
    }

    fn encrypt_payload(document: &mut XmlDocument, parent: XmlNode, key: &[u8; 32], payload: &str) {
        let mut plain = payload.as_bytes().to_vec();
        let padding = AES_BLOCK_SIZE - (plain.len() % AES_BLOCK_SIZE);
        plain.extend(std::iter::repeat_n(padding as u8, padding));
        let iv = [5u8; AES_BLOCK_SIZE];
        let mut crypter = Crypter::new(Cipher::aes_256_cbc(), Mode::Encrypt, key, Some(&iv))
            .expect("crypter");
        crypter.pad(false);
        let mut cipher_text = vec![0u8; plain.len() + AES_BLOCK_SIZE];
        let written = crypter.update(&plain, &mut cipher_text).expect("encrypts");
        let finalized = crypter
            .finalize(&mut cipher_text[written..])
            .expect("finalizes");
        cipher_text.truncate(written + finalized);
        let mut payload_bytes = iv.to_vec();
        payload_bytes.extend_from_slice(&cipher_text);

        let encrypted = document
            .append_element(parent, "xenc:EncryptedData")
            .expect("appends");
        let method = document
            .append_element(encrypted, "xenc:EncryptionMethod")
            .expect("appends");
        document
            .set_attribute(
                method,
                "Algorithm",
                "http://www.w3.org/2001/04/xmlenc#aes256-cbc",
            )
            .expect("sets");
        let key_info = document
            .append_element(encrypted, "KeyInfo")
            .expect("appends");
        let token_reference = document
            .append_element(key_info, "wsse:SecurityTokenReference")
            .expect("appends");
        let reference = document
            .append_element(token_reference, "wsse:Reference")
            .expect("appends");
        document
            .set_attribute(reference, "URI", "#response-key")
            .expect("sets");
        let cipher_data = document
            .append_element(encrypted, "xenc:CipherData")
            .expect("appends");
        document
            .append_text_element(
                cipher_data,
                "xenc:CipherValue",
                &encode_base64(&payload_bytes),
            )
            .expect("appends");
    }

    #[test]
    fn decrypts_encrypted_header_and_body_payloads() {
        let (mut document, secret, _) = encrypted_response_fixture();
        let report = decrypt_rst_response(&mut document, &secret).expect("verifies and decrypts");
        assert!(report.signature_verified);
        assert_eq!(report.payloads_decrypted, 2);
        let root = document.root();
        assert!(document.find_all(root, "EncryptedData").is_empty());
        assert!(document.find_all(root, "EncryptedPP").is_empty());
        let auth_info = document.find_unique(root, "AuthInfo").expect("unique AuthInfo");
        let marker = document.find_unique(auth_info, "marker").expect("unique marker");
        assert_eq!(document.text(marker), "header");
        let response = document
            .find_unique(root, "RequestSecurityTokenResponse")
            .expect("unique response");
        let marker = document.find_unique(response, "marker").expect("unique marker");
        assert_eq!(document.text(marker), "body");
    }

    #[test]
    fn rejects_tampered_ciphertext_and_tampered_signature() {
        let (mut document, secret, _) = encrypted_response_fixture();
        let cipher_value = document
            .find_all(document.root(), "CipherValue")
            .into_iter()
            .next()
            .expect("a CipherValue");
        let mut raw = decode_base64(&document.text(cipher_value)).expect("decodes");
        if let Some(byte) = raw.get_mut(20) {
            *byte ^= 1;
        }
        let tampered = encode_base64(&raw);
        document
            .set_text(cipher_value, &tampered)
            .expect("replaces ciphertext");
        assert_eq!(
            decrypt_rst_response(&mut document, &secret),
            Err(RstTransportError::SignedDigestMismatch)
        );

        let (mut document, secret, _) = encrypted_response_fixture();
        let signature_value = document
            .find_unique(document.root(), "SignatureValue")
            .expect("unique SignatureValue");
        document
            .set_text(signature_value, &encode_base64(&[0u8; 32]))
            .expect("replaces signature");
        assert_eq!(
            decrypt_rst_response(&mut document, &secret),
            Err(RstTransportError::SignatureMismatch)
        );
    }

    #[test]
    fn rejects_unsupported_encryption_algorithm() {
        let (mut document, secret, key) = encrypted_response_fixture();
        // Declare an algorithm this module refuses to guess at.
        let encrypted = document
            .find_all(document.root(), "EncryptedData")
            .into_iter()
            .next()
            .expect("an encrypted payload");
        let method = document
            .find_all(encrypted, "EncryptionMethod")
            .into_iter()
            .next()
            .expect("an encryption method");
        document
            .set_attribute(
                method,
                "Algorithm",
                "http://www.w3.org/2009/xmlenc11#aes256-gcm",
            )
            .expect("sets algorithm");
        // Repair the digest and the signature so the only remaining fault is the
        // unsupported algorithm.
        let target = document
            .descendants(document.root())
            .into_iter()
            .find(|candidate| document.element_id(*candidate) == Some("encrypted-pp"))
            .expect("signed target");
        let signed_info = document
            .find_unique(document.root(), "SignedInfo")
            .expect("unique SignedInfo");
        let digest = Sha256::digest(document.canonicalize(target).expect("canonicalizes"));
        for reference in document.children(signed_info) {
            if document.attribute(reference, "URI") == Some("#encrypted-pp") {
                let digest_value = document
                    .find_unique(reference, "DigestValue")
                    .expect("unique DigestValue");
                document
                    .set_text(digest_value, &encode_base64(&digest))
                    .expect("replaces digest");
            }
        }
        let mac = hmac_sha256(
            &key,
            &document.canonicalize(signed_info).expect("canonicalizes"),
        )
        .expect("macs");
        let signature_value = document
            .find_unique(document.root(), "SignatureValue")
            .expect("unique SignatureValue");
        document
            .set_text(signature_value, &encode_base64(&mac))
            .expect("replaces signature");
        assert_eq!(
            decrypt_rst_response(&mut document, &secret),
            Err(RstTransportError::UnsupportedEncryption)
        );
    }

    #[test]
    fn canonicalization_undeclares_a_default_namespace() {
        let document = parse(br#"<e xmlns="urn:d"><f xmlns=""><g/></f></e>"#);
        assert_eq!(
            canonical_text(&document, document.root()),
            r#"<e xmlns="urn:d"><f xmlns=""><g></g></f></e>"#
        );
    }

    #[test]
    fn canonicalization_sorts_the_xml_namespace_attribute_between_plain_and_prefixed() {
        let document =
            parse(br#"<e xmlns:x="urn:x" xml:lang="en" a="1" x:b="2"><!-- comment --><c/></e>"#);
        assert_eq!(
            canonical_text(&document, document.root()),
            r#"<e xmlns:x="urn:x" a="1" xml:lang="en" x:b="2"><c></c></e>"#
        );
    }

    #[test]
    fn canonicalization_declares_namespaces_where_they_are_used() {
        let document = parse(
            br#"<s:Envelope xmlns:s="urn:s" xmlns:ps="urn:ps" xmlns:wsu="urn:wsu"><s:Header><ps:AuthInfo Id="PPAuthInfo"><ps:IsAdmin>1</ps:IsAdmin></ps:AuthInfo><wsu:Timestamp wsu:Id="Timestamp"></wsu:Timestamp></s:Header></s:Envelope>"#,
        );
        assert_eq!(
            canonical_text(&document, document.root()),
            r#"<s:Envelope xmlns:s="urn:s"><s:Header><ps:AuthInfo xmlns:ps="urn:ps" Id="PPAuthInfo"><ps:IsAdmin>1</ps:IsAdmin></ps:AuthInfo><wsu:Timestamp xmlns:wsu="urn:wsu" wsu:Id="Timestamp"></wsu:Timestamp></s:Header></s:Envelope>"#
        );
    }

    #[test]
    fn xml_parser_rejects_hostile_shapes() {
        assert_eq!(XmlDocument::parse(b""), Err(RstTransportError::MalformedXml));
        assert_eq!(
            XmlDocument::parse(b"<a></b>"),
            Err(RstTransportError::MalformedXml)
        );
        assert_eq!(
            XmlDocument::parse(b"<a/><b/>"),
            Err(RstTransportError::MalformedXml)
        );
        assert_eq!(
            XmlDocument::parse(b"<a>&bogus;</a>"),
            Err(RstTransportError::MalformedXml)
        );
        assert_eq!(
            XmlDocument::parse(b"<a><b></a>"),
            Err(RstTransportError::MalformedXml)
        );
        let deep = format!(
            "{}{}",
            "<a>".repeat(MAX_XML_DEPTH + 1),
            "</a>".repeat(MAX_XML_DEPTH + 1)
        );
        assert_eq!(
            XmlDocument::parse(deep.as_bytes()),
            Err(RstTransportError::XmlTooDeep)
        );
        // Unbound prefixes only matter once the element is canonicalized.
        let document = parse(b"<a:r/>");
        assert_eq!(
            document.canonicalize(document.root()),
            Err(RstTransportError::UnboundNamespacePrefix)
        );
    }

    #[test]
    fn response_accessors_read_the_requested_token_and_the_binary_secret() {
        // Shape of the second (SSO) exchange: one requested token carrying a
        // BinarySecurityToken, plus the BinarySecret proof.
        let mut document = XmlDocument::new();
        let envelope = document.create_element("s:Envelope").expect("creates");
        for (prefix, uri) in ENVELOPE_NAMESPACES {
            document.declare_namespace(envelope, prefix, uri).expect("declares");
        }
        let body = document.append_element(envelope, "s:Body").expect("appends");
        let requested = document
            .append_element(body, "wst:RequestedSecurityToken")
            .expect("appends");
        document
            .append_text_element(requested, "wst:BinarySecurityToken", "ticket")
            .expect("appends");
        document
            .append_text_element(body, "wst:BinarySecret", "proof")
            .expect("appends");
        let response = RstResponse {
            http_status: 200,
            body_bytes: 128,
            document,
            decryption: None,
        };
        assert_eq!(response.http_status(), 200);
        assert_eq!(response.body_bytes(), 128);
        assert!(response.is_success());
        assert_eq!(response.fault_count(), 0);
        assert_eq!(response.requested_security_token_count(), 1);
        assert!(response.decryption().is_none());
        assert_eq!(response.binary_secret().expect("secret"), "proof");
        assert_eq!(
            response.binary_security_token().expect("ticket"),
            "ticket"
        );
        assert_eq!(
            response.requested_token_xml().expect("token xml"),
            "<wst:BinarySecurityToken>ticket</wst:BinarySecurityToken>"
        );
        let token = response.requested_token().expect("token");
        assert_eq!(response.document().local_name(token), "BinarySecurityToken");
    }

    #[test]
    fn transport_context_defaults_match_the_endpoint_contract() {
        let context = RstTransportContext::default();
        assert_eq!(context.endpoint, "https://login.live.com/RST2.srf");
        assert_eq!(context.user_agent, "MSAWindows/55");
        assert_eq!(context.content_type, "application/soap+xml");
        assert_eq!(context.max_response_bytes, 2 * 1024 * 1024);
    }

    /// Build a 4096-byte CLEP record whose 48-byte payload carries `secret` at
    /// the offset the reference client reads (`[12..44]`).
    fn clep_proof_fixture(secret: &[u8; 32]) -> Vec<u8> {
        let mut payload = vec![0x11u8; DEVICE_PROOF_CLEP_LENGTH];
        payload[DEVICE_SECRET_RANGE].copy_from_slice(secret);
        let mut schedule = vec![0u8; 4096 - 52];
        for (index, byte) in schedule.iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        let key = crate::services::store_rst::schedule_key(&schedule).expect("schedule key");
        let iv = [0u8; AES_BLOCK_SIZE];
        let mut crypter =
            Crypter::new(Cipher::aes_128_cbc(), Mode::Encrypt, &key, Some(&iv)).expect("crypter");
        crypter.pad(false);
        let mut cipher_text = vec![0u8; payload.len() + AES_BLOCK_SIZE];
        let written = crypter.update(&payload, &mut cipher_text).expect("encrypts");
        let finalized = crypter
            .finalize(&mut cipher_text[written..])
            .expect("finalizes");
        cipher_text.truncate(written + finalized);

        let mut record = vec![0u8; 4096];
        record[0..4].copy_from_slice(&4u32.to_le_bytes());
        record[4..52].copy_from_slice(&cipher_text);
        record[52..].copy_from_slice(&schedule);
        record
    }

    #[test]
    fn device_proof_secret_unwraps_the_clep_record() {
        let secret = [7u8; 32];
        let record = clep_proof_fixture(&secret);
        assert_eq!(device_proof_secret(&record).expect("unwraps"), secret);
        // A corrupted record and a wrong-sized record are refused outright.
        let mut tampered = record.clone();
        if let Some(byte) = tampered.get_mut(20) {
            *byte ^= 0xff;
        }
        assert_ne!(device_proof_secret(&tampered).expect("still a record"), secret);
        assert_eq!(
            device_proof_secret(&[0u8; 10]),
            Err(StoreRstError::UnsupportedClep)
        );
        assert_eq!(
            device_proof_secret(&[0u8; 4096]),
            Err(StoreRstError::UnsupportedClep)
        );
    }

    #[test]
    fn transport_errors_map_onto_the_device_error_type() {
        assert_eq!(
            StoreRstError::from(RstTransportError::Crypto),
            StoreRstError::Crypto
        );
        assert_eq!(
            StoreRstError::from(RstTransportError::MalformedBase64),
            StoreRstError::Crypto
        );
        assert_eq!(
            StoreRstError::from(RstTransportError::SignatureMismatch),
            StoreRstError::Exchange("RST response signature verification failed".to_string())
        );
        assert!(matches!(
            StoreRstError::from(RstTransportError::Http("connection reset".to_string())),
            StoreRstError::Exchange(message) if message.contains("connection reset")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn device_ticket_acquisition_refuses_an_empty_member_without_network() {
        let client = Client::builder().build().expect("client");
        let private_key = Rsa::generate(2048).expect("generates key");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        assert_eq!(
            runtime.block_on(acquire_device_ticket(&client, "   ", &private_key)),
            Err(StoreRstError::Exchange(
                "device member name is empty".to_string()
            ))
        );
    }
}
