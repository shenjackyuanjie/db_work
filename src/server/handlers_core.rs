mod health_point;
mod media;
mod pages;
mod records;
mod static_data;
mod tasks;
mod temperature;

pub(super) use health_point::health_point_handler;
pub(super) use media::recognition_image_handler;
pub(super) use pages::{
    admin_page_handler, analyze_page_handler, api_user_handler, cart_page_handler, health_handler,
    index_page_handler, orchard_3d_page_handler, store_page_handler, system_status_api_handler,
};
pub(super) use records::{disease_treatment_api_handler, recognition_records_api_handler};
pub(super) use static_data::{diagnose_api_handler, growth_tracking_api_handler, home_api_handler};
pub(super) use tasks::{
    add_task_api_handler, complete_task_api_handler, generate_task_from_disease_api_handler,
    generate_task_from_environment_api_handler, get_tasks_api_handler,
};
pub(super) use temperature::{post_temperature_humidity_handler, temperature_humidity_api_handler};
