mod bencode_value;
mod deserialize;
pub mod raw_value;
mod serialize;

pub use bencode_value::*;
pub use deserialize::{
    BencodeDeserializer, Error as DeserializeError,
    ErrorWithContext as DeserializeErrorWithContext, WithRawBytes, from_bytes,
    from_bytes_with_rest,
};
// CORRENT PATCH (E2): the nesting limit, for callers and their tests.
pub use deserialize::MAX_DEPTH;
pub use serialize::{Error as SerializeError, bencode_serialize_to_writer};

pub use buffers::{ByteBuf, ByteBufOwned};
