//! Time-stamps from a time-stamp authority (RFC 3161): the request, sent
//! through [`crate::net`], and the reply's checks.

use crate::Error;
use crate::asn1;
use crate::crypto::Hash;
use crate::net::Net;

/// A time-stamp token for `digest` (made with `hash`) from the authority
/// at `url`.
pub fn request(url: &str, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error> {
    request_with(&crate::net::Http, url, hash, digest)
}

/// [`request`] through `net`.
pub fn request_with(net: &dyn Net, url: &str, hash: Hash, digest: &[u8]) -> Result<Vec<u8>, Error> {
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|_| Error::new("no random numbers"))?;
    nonce[0] &= 0x7F;
    let req = asn1::seq(&[&asn1::uint(1), &asn1::seq(&[&asn1::alg(hash.oid(), true), &asn1::octets(digest)]), &asn1::uint_bytes(&nonce), &asn1::boolean(true)]);
    let reply = net.post(url.trim(), "application/timestamp-query", &req)?;
    token_of(&reply, digest, &nonce)
}

/// The token of a TimeStampResp, checked against the request.
fn token_of(reply: &[u8], digest: &[u8], nonce: &[u8]) -> Result<Vec<u8>, Error> {
    let resp = asn1::parse(reply).map_err(|_| Error::new("the time-stamp server didn't answer with a time-stamp"))?;
    let mut r = resp.children();
    let status = r.req()?;
    let mut sr = status.children();
    let code = sr.req()?.uint().unwrap_or(2);
    if code > 1 {
        let text = sr.next().and_then(|t| t.children().next()).map(|s| s.string()).unwrap_or_default();
        return Err(Error::new(format!("the time-stamp server refused the request ({code}) {text}").trim_end().to_string()));
    }
    let token = r.req()?.raw.to_vec();
    let st = crate::cms::stamp(&token)?;
    if st.imprint != digest {
        return Err(Error::new("the time-stamp is for other data"));
    }
    let trim = |b: &[u8]| -> Vec<u8> { b.iter().skip_while(|&&x| x == 0).copied().collect() };
    if st.nonce.as_deref().is_some_and(|n| trim(n) != trim(nonce)) {
        return Err(Error::new("the time-stamp answers another request"));
    }
    if !st.valid {
        return Err(Error::new("the time-stamp's signature is not valid"));
    }
    Ok(token)
}
