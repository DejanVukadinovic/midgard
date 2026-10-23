package midgard

import (
	"encoding/json"
	"fmt"
)

// Problem is an RFC 9457 problem document returned by the registry.
type Problem struct {
	Status int    `json:"status"`
	Code   string `json:"code"`
	Detail string `json:"detail"`
}

func (p *Problem) Error() string {
	return fmt.Sprintf("midgard: %d %s: %s", p.Status, p.Code, p.Detail)
}

func parseProblem(status int, body []byte) *Problem {
	p := &Problem{}
	if err := json.Unmarshal(body, p); err != nil || p.Status == 0 {
		return &Problem{Status: status, Code: "unknown", Detail: "unparseable error response"}
	}
	return p
}

// IsDenied reports an authorization failure (403): the caller must not retry or use stale data.
func IsDenied(err error) bool {
	p, ok := err.(*Problem)
	return ok && p.Status == 403
}
