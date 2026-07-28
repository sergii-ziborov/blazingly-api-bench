pub mod admin;
pub mod ingest;
pub mod ops;
pub mod public;

use crate::error::ApiError;

/// `page` / `limit` are shared by `/articles` and `/companies` and the contract
/// puts both out-of-range cases at 422, not 400.
pub fn page_limit(page: Option<u32>, limit: Option<u32>) -> Result<(u32, u32), ApiError> {
    let page = page.unwrap_or(1);
    let limit = limit.unwrap_or(20);
    if page < 1 {
        return Err(ApiError::invalid_field("page", "min", "must be >= 1"));
    }
    if !(1..=100).contains(&limit) {
        return Err(ApiError::invalid_field("limit", "range", "must be between 1 and 100"));
    }
    Ok((page, limit))
}

pub fn offset(page: u32, limit: u32) -> usize {
    (page as usize - 1).saturating_mul(limit as usize)
}
