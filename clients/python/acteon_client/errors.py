"""Error types for the Acteon client."""


class ActeonError(Exception):
    """Base exception for Acteon client errors."""

    def __init__(self, message: str):
        self.message = message
        super().__init__(message)


class ConnectionError(ActeonError):
    """Raised when unable to connect to the server."""

    def __init__(self, message: str):
        super().__init__(f"Connection error: {message}")

    def is_retryable(self) -> bool:
        return True


class HttpError(ActeonError):
    """Raised for HTTP errors."""

    def __init__(self, status: int, message: str):
        self.status = status
        super().__init__(f"HTTP {status}: {message}")

    def is_retryable(self) -> bool:
        return self.status >= 500


class ApiError(ActeonError):
    """Raised for API-level errors returned by the server."""

    def __init__(self, code: str, message: str, retryable: bool = False):
        self.code = code
        self.retryable = retryable
        super().__init__(f"API error [{code}]: {message}")

    def is_retryable(self) -> bool:
        return self.retryable


class RetryableError(ActeonError):
    """Raised by a task handler to fail the task as retryable.

    The :class:`~acteon_client.worker.Worker` treats *any* plain
    exception from a handler as retryable, so raising this is never
    required — it exists to make the intent explicit at the raise
    site (e.g. a caught upstream timeout being re-raised).
    """

    def is_retryable(self) -> bool:
        return True


class NonRetryableError(ActeonError):
    """Raised by a task handler to fail the task permanently.

    The :class:`~acteon_client.worker.Worker` fails the task with
    ``retryable=False``, so the server will not re-deliver it
    regardless of remaining attempts. Use for permanent conditions —
    malformed payloads, business-rule rejections — where retrying
    can never succeed.
    """

    def is_retryable(self) -> bool:
        return False
