// LNP/BP Core Library implementing LNPBP specifications & standards
// Written in 2020 by
//     Dr. Maxim Orlovsky <orlovsky@pandoracore.com>
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the MIT License
// along with this software.
// If not, see <https://opensource.org/licenses/MIT>.

use core::any::Any;
use std::io;
use std::sync::Arc;

pub trait Encode {
    type Error: std::error::Error;
    fn encode(&self, e: &mut impl io::Write) -> Result<usize, Self::Error>;
}

pub trait Decode
where
    Self: Sized,
{
    type Error: std::error::Error;
    fn decode(d: &mut impl io::Read) -> Result<Self, Self::Error>;
}

pub trait Unmarshall {
    type Data;
    type Error: std::error::Error;
    fn unmarshall(&self, reader: &mut impl io::Read) -> Result<Self::Data, Self::Error>;
}

pub type UnmarshallFn<E: ::std::error::Error> =
    fn(reader: &mut dyn io::Read) -> Result<Arc<dyn Any>, E>;