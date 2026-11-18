//! SRP-6a as HomeKit pairing uses it: the 3072-bit group of RFC 5054, g = 5, SHA-512. The
//! formulas follow the open-source senders (docs/spikes/airplay-protocol.md, section 2.2) and are
//! checked against the HomeKit specification's test vector.
//!
//! `num-bigint`'s arithmetic doesn't run in constant time. What it handles here is a PIN that
//! is on a TV's screen for a minute, on the local network, used once.

use aws_lc_rs::digest::{SHA512, digest};
use num_bigint::BigUint;

const N_HEX: &str = "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA6\
3B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245\
E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F2411\
7C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F\
83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08\
CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9\
DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D\
04507A33A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7\
ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864D8760273\
3EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E208E24FA074E5AB31\
43DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF";
/// Bytes of N: what public values are padded to.
pub const LEN: usize = 384;

fn n() -> BigUint {
    BigUint::parse_bytes(N_HEX.as_bytes(), 16).unwrap_or_default()
}

fn h(parts: &[&[u8]]) -> Vec<u8> {
    digest(&SHA512, &parts.concat()).as_ref().to_vec()
}

fn pad(v: &BigUint) -> Vec<u8> {
    let b = v.to_bytes_be();
    let mut out = vec![0u8; LEN.saturating_sub(b.len())];
    out.extend_from_slice(&b);
    out
}

/// The client's side of one exchange.
pub struct Client {
    a: BigUint,
    /// Our public value, [`LEN`] bytes.
    pub public: Vec<u8>,
}

/// What the exchange gives once the device's salt and public value are known.
#[derive(Debug, PartialEq, Eq)]
pub struct Session {
    /// The session key K = H(S), 64 bytes.
    pub key: Vec<u8>,
    /// Our proof (M1), to send.
    pub proof: Vec<u8>,
    /// The proof the device answers with (M2) when it knows the password too.
    pub expected: Vec<u8>,
}

impl Client {
    /// From 32 or more random bytes. `None` when they give a public value with a leading zero
    /// byte (1 in 256): implementations differ on how such a value is hashed, so another secret
    /// is tried instead.
    pub fn new(secret: &[u8]) -> Option<Client> {
        let a = BigUint::from_bytes_be(secret);
        let public = BigUint::from(5u8).modpow(&a, &n()).to_bytes_be();
        (public.len() == LEN).then_some(Client { a, public })
    }

    /// `None` for a public value no honest device sends (0 mod N), or a secret with a leading
    /// zero byte (the ambiguity above: pairing is started again).
    pub fn session(&self, user: &str, password: &str, salt: &[u8], device_public: &[u8]) -> Option<Session> {
        let n = n();
        let g = BigUint::from(5u8);
        let b = BigUint::from_bytes_be(device_public);
        if (&b % &n) == BigUint::default() {
            return None;
        }
        let k = BigUint::from_bytes_be(&h(&[&n.to_bytes_be(), &pad(&g)]));
        let inner = h(&[user.as_bytes(), b":", password.as_bytes()]);
        let x = BigUint::from_bytes_be(&h(&[salt, &inner]));
        let u = BigUint::from_bytes_be(&h(&[&self.public, &pad(&b)]));
        // S = (B - k·g^x) ^ (a + u·x) mod N, with the subtraction kept positive.
        let kgx = (&k * g.modpow(&x, &n)) % &n;
        let base = ((&b % &n) + &n - kgx) % &n;
        let s = base.modpow(&(&self.a + &u * &x), &n).to_bytes_be();
        let b_bytes = b.to_bytes_be();
        if s.len() != LEN || b_bytes.len() != LEN {
            return None;
        }
        let key = h(&[&s]);
        let hn = h(&[&n.to_bytes_be()]);
        let hg = h(&[&[5u8]]);
        let mixed: Vec<u8> = hn.iter().zip(&hg).map(|(a, b)| a ^ b).collect();
        let proof = h(&[&mixed, &h(&[user.as_bytes()]), salt, &self.public, &b_bytes, &key]);
        let expected = h(&[&self.public, &proof, &key]);
        Some(Session { key, proof, expected })
    }
}

/// N as bytes (tests).
#[cfg(test)]
pub(crate) fn modulus() -> Vec<u8> {
    n().to_bytes_be()
}

/// The device's side, for a receiver made for tests.
#[cfg(test)]
pub(crate) mod device {
    use super::*;

    pub struct Device {
        v: BigUint,
        b: BigUint,
        pub public: Vec<u8>,
    }

    impl Device {
        pub fn new(user: &str, password: &str, salt: &[u8], secret: &[u8]) -> Device {
            let (n, g) = (n(), BigUint::from(5u8));
            let x = BigUint::from_bytes_be(&h(&[salt, &h(&[user.as_bytes(), b":", password.as_bytes()])]));
            let v = g.modpow(&x, &n);
            let k = BigUint::from_bytes_be(&h(&[&n.to_bytes_be(), &pad(&g)]));
            let b = BigUint::from_bytes_be(secret);
            let public = pad(&((k * &v + g.modpow(&b, &n)) % &n));
            Device { v, b, public }
        }

        /// The session key, from the client's public value.
        pub fn key(&self, client_public: &[u8]) -> Vec<u8> {
            let n = n();
            let a = BigUint::from_bytes_be(client_public);
            let u = BigUint::from_bytes_be(&h(&[&pad(&a), &self.public]));
            let s = (a * self.v.modpow(&u, &n)).modpow(&self.b, &n);
            h(&[&s.to_bytes_be()])
        }

        /// The proof it answers a right client proof with.
        pub fn proof(client_public: &[u8], client_proof: &[u8], key: &[u8]) -> Vec<u8> {
            h(&[client_public, client_proof, key])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        (0..clean.len()).step_by(2).map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap()).collect()
    }

    /// The SRP test vector of the HomeKit Accessory Protocol specification, as reproduced in
    /// homebridge/fast-srp (`src/test/hap_test_data.ts`, `test_hap_vectors.ts`).
    #[test]
    fn the_homekit_test_vector() {
        let a = hex("60975527 035CF2AD 1989806F 0407210B C81EDC04 E2762A56 AFD529DD DA2D4393");
        let public = hex("FAB6F5D2 615D1E32 3512E799 1CC37443 F487DA60 4CA8C923 0FCB04E5 41DCE628
             0B27CA46 80B0374F 179DC3BD C7553FE6 2459798C 701AD864 A91390A2 8C93B644
             ADBF9C00 745B942B 79F9012A 21B9B787 82319D83 A1F83628 66FBD6F4 6BFC0DDB
             2E1AB6E4 B45A9906 B82E37F0 5D6F97F6 A3EB6E18 2079759C 4F684783 7B62321A
             C1B4FA68 641FCB4B B98DD697 A0C73641 385F4BAB 25B79358 4CC39FC8 D48D4BD8
             67A9A3C1 0F8EA121 70268E34 FE3BBE6F F89998D6 0DA2F3E4 283CBEC1 393D52AF
             724A5723 0C604E9F BCE583D7 613E6BFF D67596AD 121A8707 EEC46944 95703368
             6A155F64 4D5C5863 B48F61BD BF19A53E AB6DAD0A 186B8C15 2E5F5D8C AD4B0EF8
             AA4EA500 8834C3CD 342E5E0F 167AD045 92CD8BD2 79639398 EF9E114D FAAAB919
             E14E8509 89224DDD 98576D79 385D2210 902E9F9B 1F2D86CF A47EE244 635465F7
             1058421A 0184BE51 DD10CC9D 079E6F16 04E7AA9B 7CF7883C 7D4CE12B 06EBE160
             81E23F27 A231D184 32D7D1BB 55C28AE2 1FFCF005 F57528D1 5A88881B B3BBB7FE");
        let device_public = hex("40F57088 A482D4C7 733384FE 0D301FDD CA9080AD 7D4F6FDF 09A01006 C3CB6D56
             2E41639A E8FA21DE 3B5DBA75 85B27558 9BDB2798 63C56280 7B2B9908 3CD1429C
             DBE89E25 BFBD7E3C AD3173B2 E3C5A0B1 74DA6D53 91E6A06E 465F037A 40062548
             39A56BF7 6DA84B1C 94E0AE20 8576156F E5C140A4 BA4FFC9E 38C3B07B 88845FC6
             F7DDDA93 381FE0CA 6084C4CD 2D336E54 51C464CC B6EC65E7 D16E548A 273E8262
             84AF2559 B6264274 215960FF F47BDD63 D3AFF064 D6137AF7 69661C9D 4FEE4738
             2603C88E AA098058 1D077584 61B777E4 356DDA58 35198B51 FEEA308D 70F75450
             B71675C0 8C7D8302 FD7539DD 1FF2A11C B4258AA7 0D234436 AA42B6A0 615F3F91
             5D55CC3B 966B2716 B36E4D1A 06CE5E5D 2EA3BEE5 A1270E87 51DA45B6 0B997B0F
             FDB0F996 2FEE4F03 BEE780BA 0A845B1D 92714217 83AE6601 A61EA2E3 42E4F2E8
             BC935A40 9EAD19F2 21BD1B74 E2964DD1 9FC845F6 0EFC0933 8B60B6B2 56D8CAC8
             89CCA306 CC370A0B 18C8B886 E95DA0AF 5235FEF4 393020D2 B7F30569 04759042");
        let client = Client::new(&a).unwrap();
        assert_eq!(client.public, public);
        let s = client
            .session("alice", "password123", &hex("BEB25379 D1A8581E B5A72767 3A2441EE"), &device_public)
            .unwrap();
        assert_eq!(
            s.key,
            hex("5CBC219D B052138E E1148C71 CD449896 3D682549 CE91CA24 F098468F 06015BEB
                 6AF245C2 093F98C3 651BCA83 AB8CAB2B 580BBF02 184FEFDF 26142F73 DF95AC50")
        );
        assert_eq!(
            s.proof,
            hex(
                "5f7c14ab57ed0e94fd1d78c6b4dd09ed7e340b7e05d419a9fd760f6b35e523d1310777a1ae1d2826f596f3a85116cc457c7c964d4f44ded5559da818c88b617f"
            )
        );
        // A device that sends 0 (or N) would make the key the same for every password.
        assert!(client.session("alice", "password123", &[1], &[0]).is_none());
        assert!(client.session("alice", "password123", &[1], &n().to_bytes_be()).is_none());
    }
}
