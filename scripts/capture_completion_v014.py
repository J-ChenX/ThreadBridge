"""Compatibility API for bounded legacy request candidates; shared capture storage."""
from capture_completion import MAX_REPLY_BYTES, MIN_FREE_BYTES, indexed_title
from capture_completion import capture as _capture, capture_record as _capture_record


def capture_record(payload, allowed_thread, database, title=None, title_index=None,
                   storage_budget=None, min_free_bytes=MIN_FREE_BYTES):
    return _capture_record(payload, allowed_thread, database, title, title_index,
                           storage_budget, min_free_bytes, store_candidates=True)


def capture(payload, allowed_thread, database, title=None, title_index=None,
            storage_budget=None, min_free_bytes=MIN_FREE_BYTES):
    return _capture(payload, allowed_thread, database, title, title_index,
                    storage_budget, min_free_bytes, store_candidates=True)
