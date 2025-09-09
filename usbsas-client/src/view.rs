use crate::{
    components::{
        button_numpad, style_primary, style_secondary, FOOT_SIZE, HEADER_SIZE, MENU_SIZE, OPT_SIZE,
        TXT_SIZE,
    },
    Device, Message, State, Status, FSTYPES, GUI, LANGS,
};
use ::time::OffsetDateTime;
use bytesize::ByteSize;
use iced::{
    widget::{
        button, container, horizontal_space, image, progress_bar, scrollable,
        svg::{Handle, Svg},
        text, Checkbox, Column, Container, PickList, Row, Space,
    },
    Alignment, Color, ContentFit, Length,
};
use sysinfo::System;
use usbsas_proto::common::FileType;

impl GUI {
    pub fn view(&self) -> iced::Element<'_, Message> {
        let logo = Container::new(
            Svg::new(Handle::from_memory(include_bytes!(
                "../resources/img/usbsas-logo-b.svg"
            )))
            .content_fit(ContentFit::Contain),
        );

        let mut button_tools = button(self.i18n_txt("tools"));
        let mut button_faq = button(self.i18n_txt("faq"));

        match &self.state {
            State::Init | State::DevSelect => {
                button_tools = button_tools.on_press(Message::Tools);
                button_faq = button_faq.on_press(Message::Faq);
            }
            _ => (),
        }

        let upper_right = {
            let hostname_txt = text(System::host_name().unwrap_or("unknown".into()))
                .color(Color::BLACK)
                .size(TXT_SIZE);
            if let Some(path) = &self.config.menu_img {
                Container::new(
                    Row::new()
                        .push(image(path))
                        .push(Space::new(Length::Fixed(10.0), Length::Fill))
                        .push(hostname_txt)
                        .align_y(Alignment::Center),
                )
            } else {
                Container::new(hostname_txt)
            }
        };

        let menu = Container::new(
            Column::new()
                .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                .push(
                    Row::new()
                        .height(70)
                        .push(Space::new(Length::Fixed(10.0), Length::Fill))
                        .push(logo)
                        .push(Space::new(Length::Fill, Length::Fill))
                        .push(
                            text(self.config.window_title.clone().unwrap_or("USBSAS".into()))
                                .color(Color::BLACK)
                                .size(MENU_SIZE),
                        )
                        .push(Space::new(Length::Fill, Length::Fill))
                        .push(button_tools)
                        .push(Space::new(Length::Fixed(5.0), Length::Fill))
                        .push(button_faq)
                        .push(Space::new(Length::Fixed(5.0), Length::Fill))
                        .push(
                            PickList::new(LANGS, Some(self.lang.clone()), Message::LangSelect)
                                .text_shaping(text::Shaping::Advanced),
                        )
                        .push(Space::new(Length::Fill, Length::Fill))
                        .push(upper_right)
                        .push(Space::new(Length::Fixed(10.0), Length::Fill))
                        .align_y(Alignment::Center),
                )
                .push(Space::new(Length::Fill, Length::Fixed(10.0))),
        )
        .style(container::rounded_box);

        let main = match &self.state {
            State::Init | State::Reload => {
                let (txt, opacity) = if self.comm.is_some() {
                    (self.i18n_txt("init"), 1.0)
                } else {
                    (self.i18n_txt("loading"), 0.5)
                };
                let pane = Container::new(
                    Svg::new(Handle::from_memory(include_bytes!(
                        "../resources/img/init.svg"
                    )))
                    .content_fit(ContentFit::Contain)
                    .opacity(opacity),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(Alignment::Center);
                let column = Column::new()
                    .push(Space::new(Length::Fill, Length::Fixed(30.0)))
                    .push(pane)
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        text(txt)
                            .size(MENU_SIZE)
                            .width(Length::Fill)
                            .align_x(Alignment::Center),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(30.0)));
                Container::new(
                    Row::new()
                        .push(Space::new(Length::Fixed(30.0), Length::Fill))
                        .push(column)
                        .push(Space::new(Length::Fixed(30.0), Length::Fill)),
                )
            }
            State::Error(msg) => {
                let color_red = iced::Color {
                    r: 1.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                };
                let col = Column::new()
                    .push(
                        text(self.i18n_txt("error"))
                            .size(HEADER_SIZE)
                            .color(color_red),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(text(msg));
                Container::new(col)
            }
            State::Tools => {
                let mut col = Column::new();
                if self.comm.is_some() {
                    col = col
                        .push(
                            button(
                                Column::new()
                                    .push(text(self.i18n_txt("wipe_secure")).size(HEADER_SIZE))
                                    .push(text(self.i18n_txt("wipe_secure_desc")).size(TXT_SIZE)),
                            )
                            .width(Length::Fill)
                            .style(style_secondary)
                            .on_press(Message::Wipe(false)),
                        )
                        .push(
                            button(
                                Column::new()
                                    .push(text(self.i18n_txt("wipe_quick")).size(HEADER_SIZE))
                                    .push(text(self.i18n_txt("wipe_quick_desc")).size(TXT_SIZE)),
                            )
                            .width(Length::Fill)
                            .style(style_secondary)
                            .on_press(Message::Wipe(true)),
                        )
                        .push(
                            button(
                                Column::new()
                                    .push(text(self.i18n_txt("diskimg")).size(HEADER_SIZE))
                                    .push(text(self.i18n_txt("diskimg_desc")).size(TXT_SIZE)),
                            )
                            .width(Length::Fill)
                            .style(style_secondary)
                            .on_press(Message::DiskImg),
                        );
                };
                col = col.push(
                    button(text(self.i18n_txt("sysinfo")).size(HEADER_SIZE))
                        .width(Length::Fill)
                        .style(style_secondary)
                        .on_press(Message::SysInfo),
                );
                Container::new(scrollable(col))
            }
            State::Faq => {
                let mut col = Column::new();
                for i in 1..5 {
                    col = col
                        .push(
                            Row::new()
                                .push(
                                    text(self.i18n_txt(&format!("faq{i}")))
                                        .size(HEADER_SIZE)
                                        .width(Length::FillPortion(2)),
                                )
                                .push(Space::new(Length::Fixed(15.0), Length::Shrink))
                                .push(
                                    text(self.i18n_txt(&format!("faqr{i}")))
                                        .size(HEADER_SIZE)
                                        .width(Length::FillPortion(4)),
                                ),
                        )
                        .push(Space::new(Length::Fill, Length::Fixed(30.0)));
                }
                Container::new(scrollable(col))
            }
            State::SysInfo => {
                let mut col = Column::new().width(Length::Fill);

                // System
                let system = System::new_all();

                col = col
                    .push(text("System").size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(7.0)))
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .push(text("Hostname").width(Length::FillPortion(1)))
                            .push(
                                text(System::host_name().unwrap_or("unknown".into()))
                                    .width(Length::FillPortion(4)),
                            ),
                    )
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .push(text("Time").width(Length::FillPortion(1)))
                            .push(
                                text(if let Ok(time) = time::OffsetDateTime::now_local() {
                                    format!(
                                        "{:2}:{:2}:{:2} - {:2} {} {:4}",
                                        time.hour(),
                                        time.minute(),
                                        time.second(),
                                        time.day(),
                                        time.month(),
                                        time.year()
                                    )
                                } else {
                                    "unknown".into()
                                })
                                .width(Length::FillPortion(4)),
                            ),
                    )
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .push(text("Uptime").width(Length::FillPortion(1)))
                            .push({
                                let uptime = System::uptime();
                                let mut minutes = uptime.div_euclid(60);
                                let seconds = uptime - 60 * minutes;
                                let mut hours = minutes.div_euclid(60);
                                minutes -= hours * 60;
                                let days = hours.div_euclid(24);
                                hours -= days * 24;
                                text(format!("up {days} day(s), {hours:2} hour(s), {minutes:2} minute(s), {seconds:2} second(s)"))
                                    .width(Length::FillPortion(4))
                            }),
                    )
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .push(text("Load average").width(Length::FillPortion(1)))
                            .push(
                                text(format!("{}%", System::load_average().one))
                                    .width(Length::FillPortion(4)),
                            ),
                    )
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .push(text("Memory").width(Length::FillPortion(1)))
                            .push(
                                text(format!(
                                    "used: {:5}, free: {:5}, total: {:5}",
                                    ByteSize(system.used_memory()),
                                    ByteSize(system.available_memory()),
                                    ByteSize(system.total_memory())
                                ))
                                .width(Length::FillPortion(4)),
                            ),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(20.0)));

                // Storage
                col = col
                    .push(Space::new(Length::Fill, Length::Fixed(20.0)))
                    .push(text("Storage").size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(7.0)))
                    .push(
                        Row::new()
                            .push(horizontal_space().width(Length::FillPortion(9)))
                            .push(text("Used").width(Length::FillPortion(2)))
                            .push(text("Free").width(Length::FillPortion(2)))
                            .push(text("Total").width(Length::FillPortion(2))),
                    );

                for disk in sysinfo::Disks::new_with_refreshed_list().list() {
                    col = col.push(
                        Row::new()
                            .width(Length::Fill)
                            .push(
                                text(format!("{}", disk.name().to_string_lossy()))
                                    .width(Length::FillPortion(4)),
                            )
                            .push(
                                text(format!("{}", disk.mount_point().to_string_lossy()))
                                    .width(Length::FillPortion(4)),
                            )
                            .push(
                                text(format!("{}", disk.file_system().to_string_lossy()))
                                    .width(Length::FillPortion(1)),
                            )
                            .push(
                                text(format!(
                                    "{}",
                                    ByteSize(disk.total_space() - disk.available_space())
                                ))
                                .width(Length::FillPortion(2)),
                            )
                            .push(
                                text(format!("{}", ByteSize(disk.available_space())))
                                    .width(Length::FillPortion(2)),
                            )
                            .push(
                                text(format!("{}", ByteSize(disk.total_space())))
                                    .width(Length::FillPortion(2)),
                            ),
                    )
                }

                // Network
                col = col
                    .push(Space::new(Length::Fill, Length::Fixed(20.0)))
                    .push(text("Network").size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(7.0)));
                // HashMap -> BTreeMap to have a sorted list
                for (name, data) in sysinfo::Networks::new_with_refreshed_list()
                    .list()
                    .iter()
                    .collect::<std::collections::BTreeMap<&String, &sysinfo::NetworkData>>(
                ) {
                    col = col.push({
                        let mut row =
                            Row::new().push(text(name.to_string()).width(Length::FillPortion(1)));
                        let mut nets = data.ip_networks().to_vec();
                        nets.sort();
                        for ip in nets {
                            row = row.push(
                                text(format!("{}/{}", ip.addr, ip.prefix))
                                    .width(Length::FillPortion(3)),
                            );
                        }
                        row
                    });
                }
                Container::new(scrollable(col))
            }
            State::UserID => Container::new(
                text(self.i18n_txt("idreq"))
                    .size(MENU_SIZE)
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .height(Length::Fill),
            ),
            State::DownloadPin => {
                let pin_txt = if let Some(pin) = &self.download_pin {
                    pin
                } else {
                    ""
                };
                let mut col = Column::new()
                    .push(text(self.i18n_txt("askdlpin")).size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(20.0)))
                    .push(
                        Row::new()
                            .push(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .push(
                                text(pin_txt)
                                    .size(MENU_SIZE)
                                    .align_x(Alignment::Center)
                                    .align_y(Alignment::Center),
                            )
                            .push(Space::new(Length::Fill, Length::Fixed(1.0))),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(25.0)));

                let row_789 = Row::new()
                    .push(button_numpad("7", Message::UserInput(7)))
                    .push(Space::new(Length::Fixed(10.0), Length::Fill))
                    .push(button_numpad("8", Message::UserInput(8)))
                    .push(Space::new(Length::Fixed(10.0), Length::Fill))
                    .push(button_numpad("9", Message::UserInput(9)));
                let row_456 = Row::new()
                    .push(button_numpad("4", Message::UserInput(4)))
                    .push(Space::new(Length::Fixed(10.0), Length::Fill))
                    .push(button_numpad("5", Message::UserInput(5)))
                    .push(Space::new(Length::Fixed(10.0), Length::Fill))
                    .push(button_numpad("6", Message::UserInput(6)));
                let row_123 = Row::new()
                    .push(button_numpad("1", Message::UserInput(1)))
                    .push(Space::new(Length::Fixed(10.0), Length::Fill))
                    .push(button_numpad("2", Message::UserInput(2)))
                    .push(Space::new(Length::Fixed(10.0), Length::Fill))
                    .push(button_numpad("3", Message::UserInput(3)));
                let row_0 = Row::new()
                    .push(button_numpad("❌", Message::ClearInput(true)))
                    .push(Space::new(Length::Fixed(10.0), Length::Fill))
                    .push(button_numpad("0", Message::UserInput(0)))
                    .push(Space::new(Length::Fixed(10.0), Length::Fill))
                    .push(button_numpad("◀", Message::ClearInput(false)))
                    .align_y(Alignment::Center);

                let digits = Row::new()
                    .push(Space::new(Length::Fill, Length::Fixed(1.0)))
                    .push(
                        Column::new()
                            .push(row_789)
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                            .push(row_456)
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                            .push(row_123)
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                            .push(row_0)
                            .align_x(Alignment::Center),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(1.0)));

                col = col
                    .push(digits.height(Length::FillPortion(4)))
                    .push(Space::new(Length::Fill, Length::FillPortion(1)));

                Container::new(col)
            }
            State::DevSelect => {
                let mut src_devices = Column::new();
                let mut dst_devices = Column::new();
                for (id, device) in self.devices.iter() {
                    if device.is_src() {
                        let (title, description) = match device {
                            Device::Usb(usb) => (
                                usb.description.clone(),
                                format!("{} - {}", usb.manufacturer.clone(), usb.serial.clone()),
                            ),
                            Device::Network(net) => (net.title.clone(), net.description.clone()),
                            Device::Command(cmd) => (cmd.title.clone(), cmd.description.clone()),
                        };
                        src_devices = src_devices.push(
                            button(
                                Column::new()
                                    .push(text(title).size(HEADER_SIZE))
                                    .push(text(description).size(TXT_SIZE)),
                            )
                            .width(Length::Fill)
                            .style(if self.src_id == Some(*id) {
                                style_primary
                            } else {
                                style_secondary
                            })
                            .on_press(Message::SrcSelect(*id)),
                        );
                    }
                    if device.is_dst() {
                        let (title, description) = match device {
                            Device::Usb(usb) => (
                                usb.description.clone(),
                                format!("{} - {}", usb.manufacturer.clone(), usb.serial.clone()),
                            ),
                            Device::Network(net) => (net.title.clone(), net.description.clone()),
                            Device::Command(cmd) => (cmd.title.clone(), cmd.description.clone()),
                        };
                        dst_devices = dst_devices.push(
                            button(
                                Column::new()
                                    .push(text(title).size(HEADER_SIZE))
                                    .push(text(description).size(TXT_SIZE)),
                            )
                            .width(Length::Fill)
                            .style(if self.dst_id == Some(*id) {
                                style_primary
                            } else {
                                style_secondary
                            })
                            .on_press(Message::DstSelect(*id)),
                        );
                    }
                }
                Container::new(
                    Column::new()
                        .push(
                            Row::new()
                                .push(
                                    text("Source")
                                        .size(HEADER_SIZE)
                                        .width(Length::Fill)
                                        .align_x(Alignment::Center),
                                )
                                .push(Space::new(Length::Fixed(10.0), Length::Shrink))
                                .push(
                                    text("Destination")
                                        .size(HEADER_SIZE)
                                        .width(Length::Fill)
                                        .align_x(Alignment::Center),
                                ),
                        )
                        .push(Space::new(Length::Fill, Length::Fixed(20.0)))
                        .push(
                            Row::new()
                                .push(scrollable(src_devices.width(Length::Fill)))
                                .push(Space::new(Length::Fixed(10.0), Length::Shrink))
                                .push(scrollable(dst_devices.width(Length::Fill))),
                        ),
                )
            }
            State::PartSelect(parts) => {
                let col = Column::new()
                    .height(Length::Fill)
                    .push(text(self.i18n_txt("select_part")).size(HEADER_SIZE))
                    .push(Space::new(Length::Fixed(20.0), Length::Fixed(20.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fixed(20.0), Length::Fixed(20.0)))
                    .push(
                        Row::new()
                            .push(
                                text(self.i18n_txt("name"))
                                    .size(HEADER_SIZE)
                                    .width(Length::Fill),
                            )
                            .push(
                                text(self.i18n_txt("type"))
                                    .size(HEADER_SIZE)
                                    .width(Length::Fill),
                            )
                            .push(
                                text(self.i18n_txt("size"))
                                    .size(HEADER_SIZE)
                                    .width(Length::Fill),
                            ),
                    )
                    .push(Space::new(Length::Fixed(20.0), Length::Fixed(10.0)));
                let mut parts_col = Column::new();
                for (index, part) in parts.iter().enumerate() {
                    parts_col = parts_col
                        .push(
                            button(
                                Row::new()
                                    .push(text(&part.name_str).size(TXT_SIZE).width(Length::Fill))
                                    .push(text(&part.type_str).size(TXT_SIZE).width(Length::Fill))
                                    .push(
                                        text(ByteSize(part.size).to_string())
                                            .size(TXT_SIZE)
                                            .width(Length::Fill),
                                    ),
                            )
                            .style(button::secondary)
                            .on_press(Message::PartSelect(index.try_into().unwrap_or(0))),
                        )
                        .push(Space::new(Length::Fill, Length::Fixed(5.0)));
                }
                Container::new(col.push(scrollable(parts_col)))
            }
            State::ReadDir(files) => {
                let all_selected = if files.is_empty() {
                    false
                } else {
                    files.iter().all(|x| self.selected.contains(&x.path))
                };
                let mut back_button =
                    button(text("⬅").shaping(text::Shaping::Advanced).size(HEADER_SIZE))
                        .style(button::text);
                if self.current_dir != "/" {
                    back_button = back_button.on_press(Message::PreviousDir);
                }
                let mut file_column = Column::new()
                    .push(text(self.i18n_txt("src_files")).size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Row::new()
                            .push(Checkbox::new("", all_selected).on_toggle(|check| {
                                if check {
                                    Message::SelectAll(
                                        files
                                            .iter()
                                            .map(|x| x.path.clone())
                                            .collect::<Vec<String>>(),
                                    )
                                } else {
                                    Message::EmptySelect(
                                        files
                                            .iter()
                                            .map(|x| x.path.clone())
                                            .collect::<Vec<String>>(),
                                    )
                                }
                            }))
                            .push(Space::new(Length::Fixed(10.0), Length::Shrink))
                            .push(back_button)
                            .push(Space::new(Length::Fill, Length::Shrink))
                            .align_y(Alignment::Center),
                    );
                let mut selected_col = Column::new()
                    .push(text(self.i18n_txt("selected_files")).size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(40.0)));

                let mut file_list = Column::new();

                for file in files {
                    let datetime = match OffsetDateTime::from_unix_timestamp(file.timestamp) {
                        Ok(dt) => format!(
                            "{}/{}/{} {}:{}",
                            dt.day(),
                            u8::from(dt.month()),
                            dt.year(),
                            dt.hour(),
                            dt.minute()
                        ),
                        _ => "-".into(),
                    };
                    let mut file_row = Row::new()
                        .push(
                            Checkbox::new("", self.selected.contains(&file.path)).on_toggle(
                                |check| {
                                    if check {
                                        Message::SelectFile(file.path.clone())
                                    } else {
                                        Message::UnSelectFile(file.path.clone())
                                    }
                                },
                            ),
                        )
                        .align_y(Alignment::Center);
                    let path = if let Some(stripped) = file.path.strip_prefix(&self.current_dir) {
                        stripped
                    } else {
                        &file.path
                    };
                    match FileType::try_from(file.ftype) {
                        Ok(FileType::Regular) => {
                            file_row = file_row.push(
                                button(
                                    Row::new()
                                        .push(
                                            text("📄")
                                                .shaping(text::Shaping::Advanced)
                                                .size(TXT_SIZE),
                                        )
                                        .push(Space::new(Length::Fixed(5.0), Length::Shrink))
                                        .push(
                                            text(path.trim_start_matches('/'))
                                                .shaping(text::Shaping::Advanced)
                                                .size(TXT_SIZE)
                                                .width(Length::Fill),
                                        )
                                        .push(Space::new(Length::Fixed(5.0), Length::Shrink))
                                        .push(text(ByteSize(file.size).to_string()).size(TXT_SIZE))
                                        .push(Space::new(Length::Fixed(5.0), Length::Shrink))
                                        .push(text(datetime).size(TXT_SIZE)),
                                )
                                .style(button::text)
                                .on_press(Message::SelectFile(file.path.clone())),
                            )
                        }
                        Ok(FileType::Directory) => {
                            file_row = file_row.push(
                                button(
                                    Row::new()
                                        .push(
                                            text("📁")
                                                .shaping(text::Shaping::Advanced)
                                                .size(TXT_SIZE),
                                        )
                                        .push(Space::new(Length::Fixed(5.0), Length::Shrink))
                                        .push(
                                            text(path.trim_start_matches('/'))
                                                .size(TXT_SIZE)
                                                .shaping(text::Shaping::Advanced)
                                                .width(Length::Fill),
                                        )
                                        .push(Space::new(Length::Fixed(5.0), Length::Shrink))
                                        .push(text("").size(TXT_SIZE))
                                        .push(Space::new(Length::Fixed(5.0), Length::Shrink))
                                        .push(text(datetime).size(TXT_SIZE)),
                                )
                                .style(button::text)
                                .on_press(Message::ReadDir(file.path.clone())),
                            )
                        }
                        _ => (),
                    };
                    file_list = file_list
                        .push(file_row)
                        .push(Space::new(Length::Shrink, Length::Fixed(2.0)));
                }
                file_column = file_column.push(scrollable(file_list));

                let mut selected_list = Column::new().width(Length::Fill);
                for file in &self.selected {
                    selected_list = selected_list.push(
                        text(file.trim_start_matches('/'))
                            .shaping(text::Shaping::Advanced)
                            .size(TXT_SIZE),
                    );
                }
                selected_col = selected_col.push(scrollable(selected_list).anchor_bottom());

                Container::new(
                    Row::new()
                        .push(file_column.width(Length::Fill))
                        .push(Space::new(Length::Fixed(20.0), Length::Fixed(10.0)))
                        .push(selected_col.width(Length::Fill)),
                )
            }
            State::Wipe(_) => {
                let mut col = Column::new()
                    .height(Length::Fill)
                    .push(text(self.i18n_txt("selectwipe")).size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)));

                for (id, dev) in self.devices.iter().filter(|(_, dev)| dev.is_dst()) {
                    if let Device::Usb(usb) = dev {
                        let btn_txt =
                            format!("{}\n{} {}", usb.description, usb.manufacturer, usb.serial);
                        col = col.push(
                            button(text(btn_txt).size(TXT_SIZE))
                                .width(Length::Fill)
                                .style(if self.dst_id == Some(*id) {
                                    style_primary
                                } else {
                                    style_secondary
                                })
                                .on_press(Message::DstSelect(*id)),
                        );
                    };
                }
                Container::new(col)
            }
            State::DiskImg => {
                let mut col = Column::new()
                    .height(Length::Fill)
                    .push(text(self.i18n_txt("selectdiskimg")).size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)));
                for (id, dev) in self.devices.iter().filter(|(_, dev)| dev.is_src()) {
                    if let Device::Usb(usb) = dev {
                        let btn_txt =
                            format!("{}\n{} {}", usb.description, usb.manufacturer, usb.serial);
                        col = col.push(
                            button(text(btn_txt).size(TXT_SIZE))
                                .width(Length::Fill)
                                .style(if self.src_id == Some(*id) {
                                    style_primary
                                } else {
                                    style_secondary
                                })
                                .on_press(Message::SrcSelect(*id)),
                        );
                    };
                }
                Container::new(col)
            }
            State::Status(status) => {
                let title = if let Some(title) = &self.status_title {
                    self.i18n_txt(title)
                } else {
                    self.i18n_txt("transfering")
                };
                let mut col = Column::new()
                    .push(text(title).size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)));
                let mut col_messages = Column::new();
                let messages = &mut self.seen_status.iter().peekable();
                while let Some(message) = messages.next() {
                    let mut row = Row::new();
                    if messages.peek().is_none() {
                        row = row.push(
                            text("⏳")
                                .size(TXT_SIZE)
                                .shaping(text::Shaping::Advanced)
                                .align_y(Alignment::Center),
                        )
                    } else {
                        row = row.push(
                            text("✅")
                                .size(TXT_SIZE)
                                .shaping(text::Shaping::Advanced)
                                .align_y(Alignment::Center),
                        )
                    }
                    row = row
                        .push(Space::new(Length::Fixed(5.0), Length::Shrink))
                        .push(
                            text(self.i18n_txt(message.as_str_name()))
                                .size(TXT_SIZE)
                                .align_y(Alignment::Center),
                        );
                    if messages.peek().is_none() {
                        if let Status::Progress(progress) = status {
                            if progress.total != 0 {
                                row = row
                                    .push(Space::new(Length::Fixed(10.0), Length::Fixed(1.0)))
                                    .push(progress_bar(
                                        0.0..=100.0,
                                        progress.current as f32 * 100.0 / progress.total as f32,
                                    ));
                            }
                        };
                    }
                    col_messages = col_messages
                        .push(row)
                        .push(Space::new(Length::Fill, Length::Fixed(10.0)));
                }
                if let Status::Error(err) = status {
                    col_messages = col_messages.push(
                        text(format!("❌ {err}"))
                            .size(TXT_SIZE)
                            .shaping(text::Shaping::Advanced),
                    );
                };
                col = col.push(scrollable(col_messages).anchor_bottom());
                Container::new(col)
            }
            State::Done => {
                let mut row = Row::new();
                let mut col = Column::new()
                    .push(Space::new(Length::Fill, Length::Fixed(15.0)))
                    .push(text(self.i18n_txt("report")).size(HEADER_SIZE))
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                    .push(
                        Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                            .style(container::dark),
                    )
                    .push(Space::new(Length::Fill, Length::Fixed(10.0)));

                let mut col_mess = Column::new();
                for mess in &self.seen_status {
                    let row = Row::new().push(
                        text(format!("✅ {}", self.i18n_txt(mess.as_str_name())))
                            .size(TXT_SIZE)
                            .shaping(text::Shaping::Advanced),
                    );
                    col_mess = col_mess
                        .push(row)
                        .push(Space::new(Length::Fill, Length::Fixed(10.0)));
                }
                col = col.push(scrollable(col_mess));
                // Display report
                let mut col_report = Column::new();
                if let Some(report) = &self.report {
                    let color_red = iced::Color {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    };
                    if !report.error_files.is_empty() {
                        col_report = col_report
                            .push(text(self.i18n_txt("errors")).size(HEADER_SIZE))
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                            .push(
                                Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                                    .style(container::dark),
                            )
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)));
                        for fname in &report.error_files {
                            col_report =
                                col_report.push(text(fname).color(color_red).size(TXT_SIZE));
                        }
                    }
                    if !report.filtered_files.is_empty() {
                        col_report = col_report
                            .push(Space::new(Length::Fill, Length::Fixed(15.0)))
                            .push(text(self.i18n_txt("filtered")).size(HEADER_SIZE))
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                            .push(
                                Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                                    .style(container::dark),
                            )
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)));
                        for fname in &report.filtered_files {
                            col_report = col_report.push(
                                text(fname.trim_start_matches('/'))
                                    .size(TXT_SIZE)
                                    .color(color_red),
                            );
                        }
                    }
                    if !report.rejected_files.is_empty() {
                        col_report = col_report
                            .push(Space::new(Length::Fill, Length::Fixed(15.0)))
                            .push(text(self.i18n_txt("rejected")).size(HEADER_SIZE))
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                            .push(
                                Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                                    .style(container::dark),
                            )
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)));
                        for fname in &report.rejected_files {
                            col_report =
                                col_report.push(text(fname).size(TXT_SIZE).color(color_red));
                        }
                    }
                }
                row = row
                    .push(col)
                    .push(Space::new(Length::Fixed(20.0), Length::Fill))
                    .push(scrollable(col_report));
                let mut col2 = Column::new().push(row);
                if self
                    .devices
                    .iter()
                    .filter(|(id, dev)| {
                        matches!(dev, Device::Usb(_))
                            && (self.src_id == Some(*(*id)) || self.dst_id == Some(*(*id)))
                    })
                    .count()
                    > 0
                {
                    col2 = col2
                        .push(
                            Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                                .style(container::dark),
                        )
                        .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                        .push(
                            text(self.i18n_txt("rm_devs"))
                                .size(HEADER_SIZE)
                                .width(Length::Fill)
                                .align_x(Alignment::Center),
                        )
                        .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                        .push(
                            Container::new(Space::new(Length::Fill, Length::Fixed(1.0)))
                                .style(container::dark),
                        )
                        .push(Space::new(Length::Fill, Length::Fixed(10.0)));
                }
                Container::new(col2)
            }
        };

        let mut button_row = Row::new();
        if let State::DevSelect | State::Wipe(_) | State::DownloadPin = &self.state {
            if let Some(dst_id) = self.dst_id {
                if let Some(Device::Usb(_)) = self.devices.get(&dst_id) {
                    button_row = button_row
                        .push(Space::new(Length::Fixed(10.0), Length::Fixed(OPT_SIZE)))
                        .push(
                            text(format!("{}: ", self.i18n_txt("fstype")))
                                .size(OPT_SIZE)
                                .align_y(Alignment::Center),
                        )
                        .push(
                            PickList::new(FSTYPES, Some(self.fstype), Message::FsTypeSelect)
                                .text_size(OPT_SIZE),
                        );
                };
            };
        }
        let mut button_ok = if let State::Done = &self.state {
            button(self.i18n_txt("reset"))
        } else {
            button(self.i18n_txt("next"))
        };
        let mut button_nok = if let State::Error(_) = &self.state {
            button(self.i18n_txt("reset")).style(button::danger)
        } else {
            button(self.i18n_txt("cancel")).style(button::danger)
        };

        match self.state {
            State::Status(_) | State::Tools | State::PartSelect(_) | State::Faq | State::UserID => {
                button_nok = button_nok.on_press(Message::Nok)
            }
            State::Wipe(_) => {
                if self.dst_id.is_some() {
                    button_ok = button_ok.on_press(Message::Ok);
                }
                button_nok = button_nok.on_press(Message::Nok);
            }
            State::DiskImg => {
                if self.src_id.is_some() {
                    button_ok = button_ok.on_press(Message::Ok);
                }
                button_nok = button_nok.on_press(Message::Nok);
            }
            State::DevSelect => {
                if self.src_id.is_some() && self.dst_id.is_some() {
                    button_ok = button_ok.on_press(Message::Ok)
                }
                if self.src_id.is_some() || self.dst_id.is_some() {
                    button_nok = button_nok.on_press(Message::Nok);
                }
            }
            State::ReadDir(_) => {
                if !self.selected.is_empty() {
                    button_ok = button_ok.on_press(Message::Ok);
                }
                button_nok = button_nok.on_press(Message::Nok);
            }
            State::Done => {
                if self
                    .devices
                    .iter()
                    .filter(|(id, dev)| {
                        matches!(dev, Device::Usb(_))
                            && (self.src_id == Some(*(*id)) || self.dst_id == Some(*(*id)))
                    })
                    .count()
                    == 0
                {
                    button_ok = button_ok.on_press(Message::Ok);
                }
            }
            State::Reload => (),
            State::Error(_) => {
                button_nok = button_nok.on_press(Message::Nok);
            }
            _ => {
                button_nok = button_nok.on_press(Message::Nok);
                button_ok = button_ok.on_press(Message::Ok);
            }
        };

        if let Some(id) = &self.userid {
            button_row = button_row
                .push(Space::new(Length::Fixed(10.0), Length::Fixed(OPT_SIZE)))
                .push(
                    text(format!("👤 {id}"))
                        .shaping(text::Shaping::Advanced)
                        .size(HEADER_SIZE)
                        .align_y(Alignment::Center),
                )
        };
        button_row = button_row.push(
            Row::new()
                .push(Space::new(Length::Fill, Length::Shrink))
                .push(button_nok)
                .push(Space::new(Length::Fixed(20.0), Length::Shrink))
                .push(button_ok)
                .push(Space::new(Length::Fixed(40.0), Length::Shrink))
                .align_y(Alignment::Center),
        );
        let button_bar = Container::new(if matches!(self.state, State::Init) {
            Row::new()
        } else {
            button_row
        });

        let footer = Container::new(
            Row::new()
                .height(30)
                .push(Space::new(Length::Fill, Length::Shrink))
                .push(text(format!("Version: {}", self.version)).size(FOOT_SIZE))
                .push(Space::new(Length::Fill, Length::Shrink))
                .align_y(Alignment::Center),
        );

        let body = Column::new()
            .push(menu)
            .push(Space::new(Length::Fill, Length::Fixed(40.0)))
            .push(
                Row::new()
                    .push(Space::new(Length::Fixed(40.0), Length::Fill))
                    .push(
                        Column::new()
                            .push(main.height(Length::Fill))
                            .push(Space::new(Length::Fill, Length::Fixed(10.0)))
                            .push(button_bar),
                    )
                    .push(Space::new(Length::Fixed(40.0), Length::Fill)),
            )
            .push(Space::new(Length::Fill, Length::Fixed(5.0)))
            .push(footer);
        Container::new(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub fn i18n_txt(&self, txt: &str) -> &str {
        self.i18n
            .get(&self.lang)
            .expect("get lang")
            .get(&txt.to_lowercase())
            .unwrap_or_else(|| panic!("get txt: {txt}"))
    }
}
