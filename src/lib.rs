//               Copyright John Nunley, 2022.
// Distributed under the Boost Software License, Version 1.0.
//       (See accompanying file LICENSE or copy at
//         https://www.boost.org/LICENSE_1_0.txt)

//! Provides a safe wrapper over the X11 shared memory extension.

#![cfg(unix)]
#![deny(unsafe_code)]
#![allow(clippy::too_many_arguments)]

mod shm;
use std::{
    borrow::{Borrow, BorrowMut},
    iter::Extend,
    ops::{Deref, DerefMut},
};

use shm::{ShmBlock, ShmTransport};

use breadx::{
    display::Cookie,
    display::{Display, DisplayExt as _, DisplayFunctionsExt},
    protocol::{
        shm as xshm,
        xproto::{Drawable, Gcontext, Pixmap},
        Event,
    },
    Result,
};
use breadx_image::Image;

/// A segment attached to the X11 server.
pub struct ShmSegment {
    /// The block of SHM memory shared between the client and the server.
    block: ShmBlock,
    /// The segment ID used by the server to keep track of the segment.
    seg_id: xshm::Seg,
}

/// A segment attached to X11 for the purpose of receiving SHM images.
pub struct ShmBuffer {
    /// The transport to the X11 server.
    transport: ShmTransport,
    /// The segment ID used by the server to keep track of the segment.
    seg_id: xshm::Seg,
}

pub type ShmImage = Image<ShmSegment>;
pub type ShmRecvImage = Image<ShmBuffer>;

impl AsRef<[u8]> for ShmSegment {
    fn as_ref(&self) -> &[u8] {
        self.block.as_ref()
    }
}

impl AsRef<[u8]> for ShmBuffer {
    fn as_ref(&self) -> &[u8] {
        self.transport.as_ref()
    }
}

impl AsMut<[u8]> for ShmSegment {
    fn as_mut(&mut self) -> &mut [u8] {
        self.block.as_mut()
    }
}

impl AsMut<[u8]> for ShmBuffer {
    fn as_mut(&mut self) -> &mut [u8] {
        self.transport.as_mut()
    }
}

impl Borrow<[u8]> for ShmSegment {
    fn borrow(&self) -> &[u8] {
        self.block.borrow()
    }
}

impl Borrow<[u8]> for ShmBuffer {
    fn borrow(&self) -> &[u8] {
        self.transport.borrow()
    }
}

impl BorrowMut<[u8]> for ShmSegment {
    fn borrow_mut(&mut self) -> &mut [u8] {
        self.block.borrow_mut()
    }
}

impl BorrowMut<[u8]> for ShmBuffer {
    fn borrow_mut(&mut self) -> &mut [u8] {
        self.transport.borrow_mut()
    }
}

impl Deref for ShmSegment {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.block.as_ref()
    }
}

impl Deref for ShmBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.transport.as_ref()
    }
}

impl DerefMut for ShmSegment {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.block.as_mut()
    }
}

impl DerefMut for ShmBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.transport.as_mut()
    }
}

impl ShmSegment {
    /// Creates a new SHM segment and attaches it to the X11 server.
    pub fn attach(display: &mut impl Display, len: usize) -> Result<Self> {
        // first, create the underlying SHM block
        let block = ShmBlock::new(len).unwrap();

        // now, attach the block to the X11 server
        let seg_id = display.generate_xid()?;
        display.shm_attach_checked(seg_id, block.shm_id() as _, true)?;

        Ok(Self { block, seg_id })
    }

    /// Detaches the SHM segment from the server.
    pub fn detach(self, display: &mut impl Display) -> Result<()> {
        display.shm_detach_checked(self.seg_id)
    }
}

impl ShmBuffer {
    /// Creates a new SHM receiver and attaches it to the X11 server.
    pub fn attach(display: &mut impl Display, len: usize) -> Result<Self> {
        // first, create the underlying SHM block
        let block = ShmTransport::new(len).unwrap();

        // now, attach the block to the X11 server
        let seg_id = display.generate_xid()?;
        display.shm_attach_checked(seg_id, block.shm_id() as _, false)?;

        Ok(Self {
            transport: block,
            seg_id,
        })
    }

    /// Detaches the SHM segment from the server.
    pub fn detach(self, display: &mut impl Display) -> Result<()> {
        display.shm_detach_checked(self.seg_id)
    }

    #[allow(unsafe_code)]
    pub fn repopulate(&mut self) {
        unsafe {
            self.transport.repopulate();
        }
    }

    fn shm_id(&self) -> i32 {
        self.transport.shm_id()
    }
}

/// Extension traits for a normal display.
pub trait ShmDisplayExt: Display {
    /// Get an image from the server through an SHM transport.
    fn shm_get_ximage(
        &mut self,
        image: &mut ShmRecvImage,
        drawable: impl Into<Drawable>,
        x: i16,
        y: i16,
        plane_mask: u32,
    ) -> Result<xshm::GetImageReply> {
        let reply = self.shm_get_image_immediate(
            drawable.into(),
            x,
            y,
            image.width() as _,
            image.height() as _,
            plane_mask,
            image.format().format().into(),
            image.storage().shm_id() as _,
            0,
        )?;

        // SAFETY: the image is now populated
        image.storage_mut().repopulate();

        Ok(reply)
    }

    /// Send an SHM image to the server.
    ///
    /// `neh` stands for "no event handling".
    fn shm_put_ximage_neh(
        &mut self,
        image: &mut ShmImage,
        drawable: impl Into<Drawable>,
        gc: impl Into<Gcontext>,
        src_x: u16,
        src_y: u16,
        width: u16,
        height: u16,
        dest_x: i16,
        dest_y: i16,
        send_event: bool,
    ) -> Result<Cookie<()>> {
        let cookie = self.shm_put_image(
            drawable.into(),
            gc.into(),
            image.width() as _,
            image.height() as _,
            src_x,
            src_y,
            width,
            height,
            dest_x,
            dest_y,
            image.depth(),
            image.format().format().into(),
            send_event,
            image.storage().seg_id,
            0,
        )?;
        Ok(cookie)
    }

    /// `shm_put_ximage_neh` but checked.
    fn shm_put_ximage_neh_checked(
        &mut self,
        image: &mut ShmImage,
        drawable: impl Into<Drawable>,
        gc: impl Into<Gcontext>,
        src_x: u16,
        src_y: u16,
        width: u16,
        height: u16,
        dest_x: i16,
        dest_y: i16,
        send_event: bool,
    ) -> Result<()> {
        let cookie = self.shm_put_image(
            drawable.into(),
            gc.into(),
            image.width() as _,
            image.height() as _,
            src_x,
            src_y,
            width,
            height,
            dest_x,
            dest_y,
            image.depth(),
            image.format().format().into(),
            send_event,
            image.storage().seg_id,
            0,
        )?;
        self.wait_for_reply(cookie)
    }

    /// Write an SHM image to the server, but wait to confirm that
    /// it's finished.
    ///
    /// Events that are not SHM related are stored in the passed-in
    /// queue.
    fn shm_put_ximage(
        &mut self,
        image: &mut ShmImage,
        drawable: impl Into<Drawable>,
        gc: impl Into<Gcontext>,
        src_x: u16,
        src_y: u16,
        width: u16,
        height: u16,
        dest_x: i16,
        dest_y: i16,
        queue: &mut impl Extend<Event>,
    ) -> Result<()> {
        // send the image to the server
        self.shm_put_ximage_neh_checked(
            image, drawable, gc, src_x, src_y, width, height, dest_x, dest_y, true,
        )?;

        // wait for the server to acknowledge the image
        loop {
            let event = self.wait_for_event()?;
            let event = match event {
                Event::ShmCompletion(shm_event) => {
                    if shm_event.shmseg == image.storage().seg_id {
                        break;
                    }

                    // TODO: send the event back into the event queue,
                    // since we probably got an event meant for another
                    // image
                    Event::ShmCompletion(shm_event)
                }
                event => event,
            };

            queue.extend(Some(event));
        }

        Ok(())
    }

    /// Create a `Pixmap` using an `ShmTransport` as a backing storage.
    fn shm_create_pixmap_transport(
        &mut self,
        pid: Pixmap,
        drawable: Drawable,
        width: u16,
        height: u16,
        depth: u8,
        shmseg: &mut ShmBuffer,
        offset: u32,
    ) -> Result<Cookie<()>> {
        self.shm_create_pixmap(pid, drawable, width, height, depth, shmseg.seg_id, offset)
    }

    /// Create a `Pixmap` using an `ShmTransport` as a backing storage.
    fn shm_create_pixmap_transport_checked(
        &mut self,
        pid: Pixmap,
        drawable: Drawable,
        width: u16,
        height: u16,
        depth: u8,
        shmseg: &mut ShmBuffer,
        offset: u32,
    ) -> Result<()> {
        self.shm_create_pixmap_checked(pid, drawable, width, height, depth, shmseg.seg_id, offset)
    }
}

impl<D: Display + ?Sized> ShmDisplayExt for D {}

pub mod prelude {
    pub use crate::ShmDisplayExt;
}
