use crate::widgets::{
    CameraPaintable, NotificationKind, PortalPage, PortalPageExt, PortalPageImpl,
};
use ashpd::{desktop::camera, zbus};
use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::os::unix::prelude::RawFd;
use std::sync::Arc;
use std::sync::Mutex;
mod imp {
    use adw::subclass::prelude::*;
    use gtk::CompositeTemplate;

    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/belmoussaoui/ashpd/demo/camera.ui")]
    pub struct CameraPage {
        #[template_child]
        pub camera_available: TemplateChild<gtk::Label>,
        #[template_child]
        pub picture: TemplateChild<gtk::Picture>,
        pub paintable: CameraPaintable,
        #[template_child]
        pub revealer: TemplateChild<gtk::Revealer>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CameraPage {
        const NAME: &'static str = "CameraPage";
        type Type = super::CameraPage;
        type ParentType = PortalPage;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.install_action("camera.start", None, move |page, _action, _target| {
                let ctx = glib::MainContext::default();
                ctx.spawn_local(clone!(@weak page => async move {
                    page.start_stream().await;
                }));
            });
            klass.install_action("camera.stop", None, move |page, _, _| {
                page.stop_stream();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }
    impl ObjectImpl for CameraPage {
        fn constructed(&self, obj: &Self::Type) {
            self.picture.set_paintable(Some(&self.paintable));
            obj.action_set_enabled("camera.stop", false);
            self.parent_constructed(obj);
        }
    }
    impl WidgetImpl for CameraPage {}
    impl BinImpl for CameraPage {}
    impl PortalPageImpl for CameraPage {}
}

glib::wrapper! {
    pub struct CameraPage(ObjectSubclass<imp::CameraPage>) @extends gtk::Widget, adw::Bin, PortalPage;
}

impl CameraPage {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        glib::Object::new(&[]).expect("Failed to create a CameraPage")
    }

    async fn start_stream(&self) {
        let self_ = imp::CameraPage::from_instance(self);

        self.action_set_enabled("camera.stop", true);
        self.action_set_enabled("camera.start", false);
        match stream().await {
            Ok(Some(stream_fd)) => {
                let node_id = pipewire_node_id().await.unwrap();
                self_.paintable.set_pipewire_node_id(stream_fd, node_id);
                self_.revealer.set_reveal_child(true);
                self_.camera_available.set_text("Yes");

                self.send_notification(
                    "Camera stream started successfully",
                    NotificationKind::Success,
                );
            }
            Ok(None) => {
                self_.camera_available.set_text("No");
                self.send_notification("No camera seems to be available", NotificationKind::Info);
                self.action_set_enabled("camera.start", false);
                self.action_set_enabled("camera.stop", false);
            }
            Err(err) => {
                tracing::error!("Failed to start a camera stream {:#?}", err);
                self.send_notification(
                    "Request to start a camera stream failed",
                    NotificationKind::Error,
                );
                self.stop_stream();
            }
        }
    }

    fn stop_stream(&self) {
        let self_ = imp::CameraPage::from_instance(self);
        self.action_set_enabled("camera.stop", false);
        self.action_set_enabled("camera.start", true);

        self_.paintable.close_pipeline();
        self_.revealer.set_reveal_child(false);
    }
}

async fn stream() -> ashpd::Result<Option<RawFd>> {
    let connection = zbus::azync::Connection::session().await?;
    let proxy = camera::CameraProxy::new(&connection).await?;
    if proxy.is_camera_present().await? {
        proxy.access_camera().await?;

        Ok(Some(proxy.open_pipe_wire_remote().await?))
    } else {
        Ok(None)
    }
}

pub async fn pipewire_node_id() -> Option<u32> {
    let (sender, receiver) = futures::channel::oneshot::channel();

    let sender = Arc::new(Mutex::new(Some(sender)));
    std::thread::spawn(move || {
        let inner_sender = sender.clone();
        if let Err(err) = pipewire_node_id_inner(move |node_id| {
            if let Ok(mut guard) = inner_sender.lock() {
                if let Some(inner_sender) = guard.take() {
                    let _result = inner_sender.send(Some(node_id));
                }
            }
        }) {
            tracing::error!("Failed to get pipewire node id");
            let mut guard = sender.lock().unwrap();
            if let Some(sender) = guard.take() {
                let _ = sender.send(None);
            }
        }
    });
    receiver.await.ok().flatten()
}

fn pipewire_node_id_inner<F: FnOnce(u32) + Clone + 'static>(callback: F) -> Result<(), pw::Error> {
    use pw::prelude::*;
    let mainloop = pw::MainLoop::new()?;
    let context = pw::Context::new(&mainloop)?;
    let core = context.connect(None)?;
    let registry = core.get_registry()?;

    let loop_clone = mainloop.clone();
    let _listener_reg = registry
        .add_listener_local()
        .global(move |global| {
            if let Some(props) = &global.props {
                if props.get("media.class") == Some("Video/Source")
                    && props.get("media.role") == Some("Camera")
                {
                    callback.clone()(global.id);
                    loop_clone.quit();
                }
            }
        })
        .register();
    mainloop.run();
    Ok(())
}
