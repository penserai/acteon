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
