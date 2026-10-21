use crate::api::schema::{ErrorBody, ErrorResponse, ResponseResult, SuccessResponse};

pub(crate) fn encode_success(id: String, result: ResponseResult) -> String {
    crate::api::json_string(&SuccessResponse { id, result })
}

pub(crate) fn encode_error(id: String, code: &str, message: impl Into<String>) -> String {
    encode_error_body(
        id,
        ErrorBody {
            code: code.into(),
            message: message.into(),
        },
    )
}

pub(super) fn encode_error_body(id: String, error: ErrorBody) -> String {
    crate::api::json_string(&ErrorResponse { id, error })
}
