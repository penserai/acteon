/**
 * Error types for the Acteon client.
 */

/**
 * Base error class for Acteon client errors.
 */
export class ActeonError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ActeonError";
  }

  /**
   * Returns true if this error is retryable.
   */
  isRetryable(): boolean {
    return false;
  }
}

/**
 * Raised when unable to connect to the server.
 */
export class ConnectionError extends ActeonError {
  constructor(message: string) {
    super(`Connection error: ${message}`);
    this.name = "ConnectionError";
  }

  override isRetryable(): boolean {
    return true;
  }
}

/**
 * Raised for HTTP errors.
 */
export class HttpError extends ActeonError {
  readonly status: number;

  constructor(status: number, message: string) {
    super(`HTTP ${status}: ${message}`);
    this.name = "HttpError";
    this.status = status;
  }

  override isRetryable(): boolean {
    return this.status >= 500;
  }
}

/**
 * Raised for API-level errors returned by the server.
 */
export class ApiError extends ActeonError {
  readonly code: string;
  readonly retryable: boolean;

  constructor(code: string, message: string, retryable: boolean = false) {
    super(`API error [${code}]: ${message}`);
    this.name = "ApiError";
    this.code = code;
    this.retryable = retryable;
  }

  override isRetryable(): boolean {
    return this.retryable;
  }
}
