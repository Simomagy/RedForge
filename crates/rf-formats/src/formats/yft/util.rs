//! `Rsc8RawPtrArr`: an array of pointers whose count is elsewhere.

use crate::formats::ydr::util::read_array;
use crate::rsc8::{BlockId, Rsc8Reader, Rsc8Writer};
use crate::{Result, bail};

/// `Rsc8RawPtrArr.ReadItems(count)`: an empty array for a count of 0 (even
/// with a null pointer), the items the pointers point to otherwise (`None`
/// items for null pointers). The C# fails on a null pointer with a count.
pub fn read_raw_ptr_arr<T>(
    r: &mut Rsc8Reader,
    ptr: u64,
    count: usize,
    mut f: impl FnMut(&mut Rsc8Reader) -> Result<T>,
) -> Result<Option<Vec<Option<T>>>> {
    if count == 0 {
        return Ok(Some(Vec::new()));
    }
    let Some(ptrs) = read_array::<u64>(r, ptr, count)? else {
        bail!("a null pointer to {count} pointers (the C# fails on it)")
    };
    let mut v = Vec::with_capacity(count);
    for p in ptrs {
        v.push(r.follow(p, &mut f)?);
    }
    Ok(Some(v))
}

/// `Rsc8RawPtrArr.Write`: a null pointer for no array, a pointer to an empty
/// block for an empty one.
pub fn write_raw_ptr_arr(w: &mut Rsc8Writer, items: Option<&[Option<BlockId>]>) -> Option<BlockId> {
    items.map(|ids| w.pointer_array(ids))
}
