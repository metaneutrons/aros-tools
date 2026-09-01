//! ROM-tool facade over the shared atomic tree publisher.

use aros_common::{publish_flat_tree_noclobber, PortableOutputName, PublicationReceipt};
use std::path::Path;

pub struct NewMember<'a> {
    pub name: &'a str,
    pub contents: &'a [u8],
}

pub fn publish_new_members(
    directory: &Path,
    members: &[NewMember<'_>],
) -> std::io::Result<PublicationReceipt> {
    let validated: Vec<(PortableOutputName, &[u8])> = members
        .iter()
        .map(|member| PortableOutputName::new(member.name).map(|name| (name, member.contents)))
        .collect::<std::io::Result<_>>()?;
    publish_flat_tree_noclobber(directory, &validated)
}
