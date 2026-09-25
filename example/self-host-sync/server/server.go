package main

import (
	"errors"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

type service struct {
	passwordPath string
	auth         *authStore
	exchange     *exchangeStore
	web          webSessions
	webCSRFKey   [32]byte
}

func newService(dataDir string) (*service, error) {
	if err := os.MkdirAll(dataDir, 0700); err != nil {
		return nil, err
	}
	info, err := os.Lstat(dataDir)
	if err != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return nil, errors.New("data directory must be a real directory")
	}
	if err := os.Chmod(dataDir, 0700); err != nil {
		return nil, err
	}
	if err := requirePasswordReady(dataDir); err != nil {
		return nil, err
	}
	auth, err := openAuthStore(dataDir)
	if err != nil {
		return nil, err
	}
	exchange, err := openExchangeStore(filepath.Join(dataDir, "exchange.json"))
	if err != nil {
		return nil, err
	}
	csrfKey, err := newWebCSRFKey()
	if err != nil {
		return nil, err
	}
	return &service{passwordPath: filepath.Join(dataDir, "auth.json"), auth: auth, exchange: exchange, web: webSessions{tokens: make(map[string]webSession)}, webCSRFKey: csrfKey}, nil
}

func (s *service) routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/", s.webIndex)
	mux.HandleFunc("/web/login", postOnly(s.webLogin))
	mux.HandleFunc("/web/logout", postOnly(s.webLogout))
	mux.HandleFunc("/auth/login", postOnly(func(w http.ResponseWriter, r *http.Request) { s.auth.login(w, r, s.passwordPath) }))
	mux.HandleFunc("/auth/token", postOnly(func(w http.ResponseWriter, r *http.Request) { s.auth.refresh(w, r, s.passwordPath) }))
	mux.HandleFunc("/auth/revoke", postOnly(func(w http.ResponseWriter, r *http.Request) { s.auth.revokeHandler(w, r, s.passwordPath) }))
	mux.HandleFunc("/auth/register", postOnly(func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "registration disabled", http.StatusForbidden)
	}))
	mux.HandleFunc("/auth/email/verify", postOnly(func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "verification disabled", http.StatusBadRequest)
	}))
	mux.HandleFunc("/auth/mfa", postOnly(func(w http.ResponseWriter, _ *http.Request) { http.Error(w, "MFA disabled", http.StatusBadRequest) }))
	mux.HandleFunc("/auth/authorize", func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "browser authorization disabled", http.StatusMethodNotAllowed)
	})
	mux.HandleFunc("/exchange", s.exchangeHandler)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Cache-Control", "no-store")
		w.Header().Set("X-Content-Type-Options", "nosniff")
		if r.URL.RawQuery != "" {
			http.Error(w, "query parameters are not accepted", http.StatusBadRequest)
			return
		}
		mux.ServeHTTP(w, r)
	})
}

func postOnly(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			w.Header().Set("Allow", "POST")
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		next(w, r)
	}
}

func (s *service) exchangeHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet && r.Method != http.MethodPut && r.Method != http.MethodDelete {
		w.Header().Set("Allow", "GET, PUT, DELETE")
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	if !s.auth.requireAccess(w, r, s.passwordPath) {
		return
	}
	switch r.Method {
	case http.MethodGet:
		s.getExchange(w)
	case http.MethodPut:
		s.putExchange(w, r)
	case http.MethodDelete:
		s.deleteExchange(w, r)
	}
}

func (s *service) getExchange(w http.ResponseWriter) {
	state, poisoned := s.exchange.get()
	if poisoned {
		http.Error(w, "storage unavailable", http.StatusServiceUnavailable)
		return
	}
	if state.Revision == 0 {
		w.Header().Set("X-NoriShell-Next-Revision", strconv.FormatUint(state.NextRevision, 10))
		w.WriteHeader(http.StatusNotFound)
		return
	}
	setExchangeHeaders(w, state.ETag, state.Revision, state.UpdatedMS)
	w.Header().Set("Content-Type", contentType)
	w.Header().Set("Content-Length", strconv.Itoa(len(state.Body)))
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write(state.Body)
}

func (s *service) putExchange(w http.ResponseWriter, r *http.Request) {
	if r.Header.Get("Content-Type") != contentType {
		http.Error(w, "invalid content type", http.StatusBadRequest)
		return
	}
	key := r.Header.Get("Idempotency-Key")
	if !validUUID(key) {
		http.Error(w, "invalid idempotency key", http.StatusBadRequest)
		return
	}
	condition, ok := requestCondition(r, true)
	if !ok {
		http.Error(w, "exactly one write condition is required", http.StatusBadRequest)
		return
	}
	if r.ContentLength > maxExchangeBytes {
		http.Error(w, "exchange too large", http.StatusRequestEntityTooLarge)
		return
	}
	body, err := boundedBody(r.Body, maxExchangeBytes)
	if err != nil {
		status := http.StatusBadRequest
		if err.Error() == "body too large" {
			status = http.StatusRequestEntityTooLarge
		}
		http.Error(w, "invalid exchange body", status)
		return
	}
	if len(body) == 0 {
		http.Error(w, "exchange body required", http.StatusBadRequest)
		return
	}
	entry, status := s.exchange.mutate("PUT", key, digestHex(body), condition, body)
	if status != 200 && status != 201 {
		http.Error(w, http.StatusText(status), status)
		return
	}
	setExchangeHeaders(w, entry.ETag, entry.Revision, entry.UpdatedMS)
	w.WriteHeader(status)
}

func (s *service) deleteExchange(w http.ResponseWriter, r *http.Request) {
	key := r.Header.Get("Idempotency-Key")
	if !validUUID(key) {
		http.Error(w, "invalid idempotency key", http.StatusBadRequest)
		return
	}
	condition, ok := requestCondition(r, false)
	if !ok {
		http.Error(w, "strong If-Match required", http.StatusBadRequest)
		return
	}
	entry, status := s.exchange.mutate("DELETE", key, "", condition, nil)
	if status != 204 {
		http.Error(w, http.StatusText(status), status)
		return
	}
	_ = entry
	w.WriteHeader(http.StatusNoContent)
}

func requestCondition(r *http.Request, allowNext bool) (string, bool) {
	match := r.Header.Get("If-Match")
	next := r.Header.Get("X-NoriShell-Expected-Next-Revision")
	if match != "" {
		if next != "" || len(match) < 2 || len(match) > 1024 || !strings.HasPrefix(match, "\"") || !strings.HasSuffix(match, "\"") || strings.ContainsAny(match, "\r\n") {
			return "", false
		}
		return "etag:" + match, true
	}
	if !allowNext || next == "" || len(next) > 16 || strings.Trim(next, "0123456789") != "" {
		return "", false
	}
	value, err := strconv.ParseUint(next, 10, 64)
	if err != nil || value == 0 || value > maxRevision || strconv.FormatUint(value, 10) != next {
		return "", false
	}
	return "next:" + next, true
}

func setExchangeHeaders(w http.ResponseWriter, etag string, revision uint64, updatedMS int64) {
	w.Header().Set("ETag", etag)
	w.Header().Set("X-NoriShell-Revision", fmt.Sprint(revision))
	w.Header().Set("X-NoriShell-Updated-At-Unix-Ms", fmt.Sprint(updatedMS))
}
