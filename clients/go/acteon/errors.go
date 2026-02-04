package acteon

import "fmt"

// ActeonError is the base interface for Acteon client errors.
type ActeonError interface {
	error
	IsRetryable() bool
}

// ConnectionError represents a connection failure.
type ConnectionError struct {
	Message string
}

func (e *ConnectionError) Error() string {
	return fmt.Sprintf("connection error: %s", e.Message)
}

func (e *ConnectionError) IsRetryable() bool {
	return true
}

// HTTPError represents an HTTP error.
type HTTPError struct {
	Status  int
	Message string
}

func (e *HTTPError) Error() string {
	return fmt.Sprintf("HTTP %d: %s", e.Status, e.Message)
}

func (e *HTTPError) IsRetryable() bool {
	return e.Status >= 500
}

// APIError represents an API-level error returned by the server.
type APIError struct {
	Code      string
	Message   string
	Retryable bool
}

func (e *APIError) Error() string {
	return fmt.Sprintf("API error [%s]: %s", e.Code, e.Message)
}

func (e *APIError) IsRetryable() bool {
	return e.Retryable
}
