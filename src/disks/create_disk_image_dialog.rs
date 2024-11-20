use std::ffi::CString;
use std::io::{ErrorKind, Read};
use std::ops::Sub;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

use adw::prelude::*;
use gettextrs::{gettext, pgettext};
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use libgdu::gettext::gettext_f;

use crate::estimator::GduEstimator;
use crate::page_buffer::PageAlignedBuffer;

mod imp {
    use std::cell::RefCell;

    use adw::subclass::window::AdwWindowImpl;

    use crate::config;

    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(file = "ui/gdu-create-disk-image-dialog.ui")]
    pub struct GduCreateDiskImageDialog {
        pub(super) client: RefCell<Option<udisks::Client>>,
        pub(super) object: RefCell<Option<udisks::Object>>,
        pub(super) block: RefCell<Option<udisks::block::BlockProxy<'static>>>,
        pub(super) drive: RefCell<Option<udisks::drive::DriveProxy<'static>>>,
        pub(super) directory_path: RefCell<std::path::PathBuf>,

        #[template_child]
        pub(super) name_entry: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub(super) location_entry: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub(super) source_label: TemplateChild<adw::ActionRow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GduCreateDiskImageDialog {
        const NAME: &'static str = "GduCreateDiskImageDialog";
        type Type = super::GduCreateDiskImageDialog;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_instance_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for GduCreateDiskImageDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Devel Profile
            if config::PROFILE == "Devel" {
                obj.add_css_class("devel");
            }
        }

        fn dispose(&self) {
            self.dispose_template();
        }
    }

    impl WidgetImpl for GduCreateDiskImageDialog {}
    impl WindowImpl for GduCreateDiskImageDialog {}
    impl AdwWindowImpl for GduCreateDiskImageDialog {}
}

glib::wrapper! {
    pub struct GduCreateDiskImageDialog(ObjectSubclass<imp::GduCreateDiskImageDialog>)
        @extends gtk::Widget, gtk::Window, adw::Window,
        @implements gio::ActionMap, gio::ActionGroup, gtk::Root;
}

#[gtk::template_callbacks]
impl GduCreateDiskImageDialog {
    pub async fn show(
        parent_window: &impl IsA<gtk::Window>,
        object: udisks::Object,
        client: udisks::Client,
    ) {
        let dialog: Self = glib::Object::builder()
            .property("application", parent_window.application())
            .property("transient-for", parent_window)
            .build();
        let imp = dialog.imp();
        let block = object.block().await.expect("`object` should be a block");
        imp.drive.replace(client.drive_for_block(&block).await.ok());
        dialog.set_default_name(&block).await;
        imp.block.replace(Some(block));

        // update source label
        if let Some(info) = client.object_info(&object).await.one_liner {
            imp.source_label.set_subtitle(&info);
        }
        imp.client.replace(Some(client));
        imp.object.replace(Some(object));

        let directory_path =
            glib::user_special_dir(glib::UserDirectory::Documents).unwrap_or_default();
        dialog.update_directory(directory_path);

        dialog.present();
    }

    fn client(&self) -> udisks::Client {
        self.imp().client.borrow().clone().unwrap()
    }

    async fn set_default_name(&self, block: &udisks::block::BlockProxy<'static>) -> Option<()> {
        let device_name = block
            .preferred_device()
            .await
            .ok()
            .and_then(|dev| CString::from_vec_with_nul(dev).ok())
            .and_then(|dev| dev.to_str().map(|p| p.to_string()).ok())?
            .replacen("/dev/", "", 1)
            .replace("/", "_");

        let fstype = block.id_type().await.unwrap_or_default();
        let fslabel = block.id_label().await.unwrap_or_default();
        let proposed_filename = if (fstype == "iso9660" || fstype == "udf") && !fslabel.is_empty() {
            format!("{}.iso", fslabel)
        } else {
            let now = glib::DateTime::now(&glib::TimeZone::local()).ok()?;

            // Translators: The suggested name for the disk image to create.
            // The first %s is a name for the disk (e.g. 'sdb').
            // The second %s is today's date and time, e.g. "March 2, 1976 6:25AM".
            gettext_f(
                "Disk Image of {} ({}).img",
                [&device_name, &now.format("%Y-%m-%d %H%M").ok()?.into()],
            )
        };
        self.imp().name_entry.set_text(&proposed_filename);
        None
    }

    fn update_directory(&self, path: PathBuf) {
        let imp = self.imp();
        let unfused_path = libgdu::unfuse_path(&path);
        imp.directory_path.replace(path);
        imp.location_entry.set_subtitle(&unfused_path);
    }

    fn play_complete_sound(&self) {
        // Translators: A descriptive string for the 'complete' sound, see CA_PROP_EVENT_DESCRIPTION
        let _sound_message = gettext("Disk image copying complete");
        /* gtk4 todo : Find a replacement for this
        ca_gtk_play_for_widget (GTK_WIDGET (self->dialog), 0,
                                CA_PROP_EVENT_ID, "complete",
                                CA_PROP_EVENT_DESCRIPTION, sound_message,
                                NULL);
        */
    }

    #[template_callback]
    async fn on_choose_folder_button_clicked_cb(&self) {
        let directory_path = self.imp().directory_path.borrow().clone();
        let file_dialog = gtk::FileDialog::builder()
            .title(gettext("Choose a location to save the disk image."))
            .initial_folder(&gio::File::for_path(directory_path))
            .build();
        if let Some(file_path) = file_dialog
            .select_folder_future(Some(self))
            .await
            .ok()
            .and_then(|file| file.path())
        {
            self.update_directory(file_path);
        }
    }

    #[template_callback]
    async fn on_create_image_button_clicked_cb(&self, _button: &gtk::Button) {
        let name = self.imp().name_entry.text();
        let directory = self.imp().directory_path.borrow().clone();
        let file = directory.join(&name);
        if !file.exists() {
            self.create_disk_image().await;
            return;
        }

        let confirmation_dialog = libgdu::ConfirmationDialog {
            message: gettext("Replace File?"),
            description: gettext_f(
                "A file named “{}” already exists in {}",
                [name.as_str(), &libgdu::unfuse_path(&directory.as_path())],
            ),
            reponse_verb: gettext("Replace"),
            reponse_appearance: adw::ResponseAppearance::Destructive,
        };
        let response = confirmation_dialog.show(self, gtk::Widget::NONE).await;
        if response == libgdu::ConfirmationDialogResponse::Cancel {
            return;
        }
        self.create_disk_image().await;
    }

    pub async fn create_disk_image(&self) -> Option<()> {
        let imp = self.imp();
        // it's fine to steal the values here, since the dialog will close after this operation
        let object = imp.object.take()?;
        let block = imp.block.take()?;
        let drive = imp.drive.take()?;
        let device = block
            .device()
            .await
            .ok()
            .and_then(|dev| CString::from_vec_with_nul(dev).ok())
            .and_then(|dev| dev.to_str().map(|p| p.to_string()).ok())?;
        if !device.starts_with("/dev/sr") {
            libgdu::ensure_unused(&self.client(), self, &object)
                .await
                .expect("`object` should be unused");
        }

        let name = imp.name_entry.text();
        let mut output_file_path = imp.directory_path.take();
        output_file_path.push(&name);
        let mut output_file = match std::fs::File::create(&output_file_path) {
            Ok(file) => file,
            Err(err) => {
                libgdu::show_error(
                    self,
                    &gettext("Error opening file for writing"),
                    Box::new(err),
                );
                return None;
            }
        };

        let application = self.application().unwrap_or_default();
        let inhibit_cookie = application.inhibit(
            self.native().and_downcast_ref::<gtk::Window>(),
            gtk::ApplicationInhibitFlags::SUSPEND | gtk::ApplicationInhibitFlags::LOGOUT,
            // Translators: Reason why suspend/logout is being inhibited
            Some(&pgettext(
                "create-inhibit-message",
                "Copying device to disk image",
            )),
        );
        //TODO: create job

        let copy_res = self
            .copy_device(
                &device,
                &block,
                &drive,
                &mut output_file,
            )
            .await;

        application.uninhibit(inhibit_cookie);

        if let Err(err) = copy_res {
            libgdu::show_error(self, &gettext("Error creating disk image"), err);
            //TODO: use same return as happy path
            self.set_visible(false);
            self.close();
            return None;
        };

        self.play_complete_sound();
        self.update_job(None, true);

        let (zero_bytes, block_size) = copy_res.unwrap();
        if zero_bytes > 0 {
            let percentage = 100.0 * zero_bytes as f64 / block_size as f64;
            //TODO: also show this when another error occured?
            //TODO: why even continue to copy the disk, instead of exiting early?
            let response = libgdu::ConfirmationDialog {
                // Translators: Heading in dialog shown if some data was unreadable while creating a disk image
                message: gettext("Unrecoverable read errors"),
                // Translators: Body in dialog shown if some data was unreadable while creating a disk image.
                // The %f is the percentage of unreadable data (ex. 13.0).
                // The first %s is the amount of unreadable data (ex. "4.2 MB").
                // The second %s is the name of the device (ex "/dev/").
                description: gettext_f("{:2.1}% ({}) of the data on the device “{}” was unreadable and replaced with zeroes in the created disk image file. This typically happens if the medium is scratched or if there is physical damage to the drive", [percentage.to_string(), zero_bytes.to_string(), self.imp().source_label.subtitle().unwrap().into()]),
                reponse_verb: gettext("_Delete Disk Image File"),
                reponse_appearance: adw::ResponseAppearance::Destructive
            }.show(self, gtk::Widget::NONE).await;

            if response == libgdu::ConfirmationDialogResponse::Cancel {
                return None;
            }

            //TODO: use async remove?
            if let Err(err) = std::fs::remove_file(&output_file_path) {
                log::error!(
                    "Error deleting file: {} ({})",
                    output_file_path.display(),
                    err
                );
            }
        }

        self.set_visible(false);
        self.close();
        None
    }

    async fn copy_device(
        &self,
        device: &str,
        block: &udisks::block::BlockProxy<'static>,
        drive: &udisks::drive::DriveProxy<'static>,
        output_file: &mut impl std::io::Write,
    ) -> Result<(usize, u64), Box<dyn std::error::Error>> {
        let Ok(mut device) = (if device.starts_with("/dev/sr") {
            let file = std::fs::File::open(device);
            if block.id_usage().await.is_ok_and(|id| id == "filesystem")
                && block.id_type().await.is_ok_and(|id| id == "udf")
                && drive
                    .media()
                    .await
                    .is_ok_and(|media| media == "optical_drive")
            {
                todo!("Handle libdvdcss");
            }
            file
        } else {
            // request the file from udisks directly
            let fd: std::os::fd::OwnedFd = block
                .open_for_backup(std::collections::HashMap::new())
                .await?
                .into();
            let file = std::fs::File::from(fd);
            Ok(file)
        }) else {
            return Err(Box::new(std::io::Error::from(
                std::io::ErrorKind::InvalidData,
            )));
        };

        // We can't use udisks_block_get_size() because the media may have
        // changed and udisks may not have noticed. TODO: maybe have a
        // Block.GetSize() method instead...

        // https://github.com/topjohnwu/Magisk/blob/33f70f8f6df24f66f7da9ad855cd4f7fe72c37a9/native/src/base/files.rs#L908
        #[cfg(target_pointer_width = "32")]
        const BLKGETSIZE64: u64 = 0x80041272;
        #[cfg(target_pointer_width = "64")]
        const BLKGETSIZE64: u64 = 0x80081272;

        // TODO: this is also used in restore dialog, abstract this to a common implementation
        let mut block_device_size: u64 = 0;
        if unsafe { libc::ioctl(device.as_raw_fd(), BLKGETSIZE64, &mut block_device_size) } != 0 {
            log::error!("Error determining size of device");
            return Err(Box::new(std::io::Error::from(
                std::io::ErrorKind::InvalidData,
            )));
        }

        if block_device_size == 0 {
            log::error!("Device is size 0");
            return Err(Box::new(std::io::Error::from(
                std::io::ErrorKind::InvalidData,
            )));
        }

        match allocate_file_size(output_file, block_device_size as i64) {
            // kernel or filesystem does not support fallocate, ignore
            Ok(()) | Err(libc::ENOSYS) | Err(libc::EOPNOTSUPP) => {
                log::debug!("`fallocate` successful");
            }
            Err(err) => {
                log::error!("Failed to fallocate file: {err}");
                return Err(Box::new(std::io::Error::from(
                    std::io::ErrorKind::InvalidData,
                )));
            }
        };

        // default to 1 MiB blocks
        const BUFFER_SIZE: usize = 1024 * 1024;
        let mut page_buffer = PageAlignedBuffer::new(BUFFER_SIZE);
        let buffer = page_buffer.as_mut_slice();

        let estimator = GduEstimator::new(block_device_size);

        // Read huge (e.g. 1 MiB) blocks and write it to the output file even if it was only
        // partially read
        let mut bytes_completed = 0;
        let update_interval = std::time::Duration::from_millis(200);
        // set initial timer back by the update interval, so the UI is refreshed on the first cycle
        let update_timer = std::time::Instant::now().sub(update_interval);
        let mut padded_bytes = 0;
        loop {
            // Update GUI - but only every 200ms and if the last update isn't peding
            if update_timer.elapsed() >= update_interval {
                if bytes_completed > 0 {
                    estimator.add_sample(bytes_completed);
                }
                //TODO: add a progress bar?
                //TODO: update
            }

            //TODO: check if using kernel calls like std's (file) copy does is faster
            //or using BufWriter
            //or BufReader
            let read_bytes = match device.read(buffer) {
                // we finished reading all bytes
                Ok(0) => break,
                Ok(n) if n < buffer.len() => {
                    // if we read less bytes than expected, pad the rest with 0
                    // TODO: check if this is correct, or an off-by-one error
                    buffer[n..].fill(0);
                    padded_bytes += buffer.len() - n;
                    buffer.len()
                }
                Ok(n) => n,
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) => return Err(Box::new(err)),
            };

            if let Err(err) = output_file.write_all(&buffer[..read_bytes]) {
                log::error!("Error writing to device: {}", err);
                return Err(Box::new(err));
            }
            bytes_completed += read_bytes as u64;
        }
        log::info!("successfully copied disk image");

        Ok((padded_bytes, block_device_size))
    }

    fn update_job(&self, estimator: Option<&GduEstimator>, done: bool) {
        let (bytes_per_sec, usec_remaining, completed_bytes, target_bytes) =
            if let Some(estimator) = estimator {
                (
                    estimator.bytes_per_sec(),
                    estimator.usec_remaining(),
                    estimator.completed_bytes(),
                    estimator.target_bytes(),
                )
            } else {
                (0, 0, 0, 0)
            };
        //TODO: update job
    }
}

/// Allocates `size` disk space for `file`.
///
/// # Errors
///
/// Returns the error code of the underlying `fallocate` call.
fn allocate_file_size(file: &mut std::fs::File, size: i64) -> Result<(), i32> {
    if unsafe { libc::fallocate(file.as_raw_fd(), 0, 0, size) } != 0 {
        return Err(std::io::Error::last_os_error()
            .raw_os_error()
            .expect("`last_os_error` must be set as `fallocate` failed"));
    }
    Ok(())
}
