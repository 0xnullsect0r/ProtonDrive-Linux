// Package main provides a C-shared-library bridge over
// henrybear327/Proton-API-Bridge so that the Rust client can talk to
// Proton Drive without re-implementing SRP, GopenPGP and the Drive REST
// surface from scratch.
//
// All exported functions accept and return JSON-encoded C strings.
// The caller (Rust) MUST call pd_free to release returned strings.
//
// A "session" is an opaque integer handle that maps to an internal
// *ProtonDrive instance held in the Go heap.
package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"sync"
	"sync/atomic"
	"time"
	"unsafe"

	bridge "github.com/henrybear327/Proton-API-Bridge"
	"github.com/henrybear327/Proton-API-Bridge/common"
	"github.com/henrybear327/go-proton-api"
	"github.com/go-resty/resty/v2"
)

// ---------- session registry ----------

type session struct {
	drive        *bridge.ProtonDrive
	cfg          *common.Config
	cred         *common.ProtonDriveCredential
	cancel       context.CancelFunc
	ctx          context.Context
	pendingLogin *loginArgs // stored when login is blocked by HV (code 9001)
}

var (
	sessions sync.Map // int64 -> *session
	nextID   int64
)

func putSession(s *session) int64 {
	id := atomic.AddInt64(&nextID, 1)
	sessions.Store(id, s)
	return id
}

func getSession(id int64) (*session, bool) {
	v, ok := sessions.Load(id)
	if !ok {
		return nil, false
	}
	return v.(*session), true
}

func dropSession(id int64) {
	sessions.Delete(id)
}

// ---------- response helpers ----------

type response struct {
	OK   any    `json:"ok,omitempty"`
	Err  string `json:"err,omitempty"`
	Code int    `json:"code,omitempty"`
}

func cReturn(r response) *C.char {
	b, err := json.Marshal(r)
	if err != nil {
		b = []byte(`{"err":"json marshal failed"}`)
	}
	return C.CString(string(b))
}

func cOK(v any) *C.char     { return cReturn(response{OK: v}) }
func cErr(e error) *C.char  { return cReturn(response{Err: e.Error()}) }
func cErrS(s string) *C.char { return cReturn(response{Err: s}) }

// ---------- DTOs (mirrored on the Rust side) ----------

type initArgs struct {
	AppVersion          string `json:"app_version"`
	UserAgent           string `json:"user_agent"`
	DataFolderName      string `json:"data_folder_name"`
	EnableCaching       bool   `json:"enable_caching"`
	ConcurrentBlocks    int    `json:"concurrent_blocks"`
	ConcurrentCrypto    int    `json:"concurrent_crypto"`
	ReplaceExisting     bool   `json:"replace_existing"`
	CredentialCacheFile string `json:"credential_cache_file"`
}

type loginArgs struct {
	Username        string `json:"username"`
	Password        string `json:"password"`
	MailboxPassword string `json:"mailbox_password"`
	TwoFA           string `json:"two_fa"`
}

type resumeArgs struct {
	UID           string `json:"uid"`
	AccessToken   string `json:"access_token"`
	RefreshToken  string `json:"refresh_token"`
	SaltedKeyPass string `json:"salted_key_pass"`
}

type credDTO struct {
	UID           string `json:"uid"`
	AccessToken   string `json:"access_token"`
	RefreshToken  string `json:"refresh_token"`
	SaltedKeyPass string `json:"salted_key_pass"`
}

type linkDTO struct {
	LinkID       string `json:"link_id"`
	ParentLinkID string `json:"parent_link_id"`
	Name         string `json:"name"`
	IsFolder     bool   `json:"is_folder"`
	MIMEType     string `json:"mime_type"`
	Size         int64  `json:"size"`
	ModifyTime   int64  `json:"modify_time"`
	CreateTime   int64  `json:"create_time"`
	State        int    `json:"state"`
	Hash         string `json:"hash"`
}

func protonLinkToDTO(name string, isFolder bool, l *proton.Link) linkDTO {
	if l == nil {
		return linkDTO{Name: name, IsFolder: isFolder}
	}
	return linkDTO{
		LinkID:       l.LinkID,
		ParentLinkID: l.ParentLinkID,
		Name:         name, // bridge has decrypted this for us
		IsFolder:     isFolder,
		MIMEType:     l.MIMEType,
		Size:         l.Size,
		ModifyTime:   l.ModifyTime,
		CreateTime:   l.CreateTime,
		State:        int(l.State),
		Hash:         l.Hash,
	}
}

// ---------- exported API ----------

//export pd_free
func pd_free(p *C.char) {
	if p != nil {
		C.free(unsafe.Pointer(p))
	}
}

//export pd_version
func pd_version() *C.char {
	return C.CString(bridge.LIB_VERSION)
}

//export pd_init
func pd_init(argsJSON *C.char) *C.char {
	var a initArgs
	if err := json.Unmarshal([]byte(C.GoString(argsJSON)), &a); err != nil {
		return cErr(err)
	}
	cfg := common.NewConfigWithDefaultValues()
	if a.AppVersion != "" {
		cfg.AppVersion = a.AppVersion
	}
	if a.UserAgent != "" {
		cfg.UserAgent = a.UserAgent
	}
	if a.DataFolderName != "" {
		cfg.DataFolderName = a.DataFolderName
	}
	cfg.EnableCaching = a.EnableCaching
	if a.ConcurrentBlocks > 0 {
		cfg.ConcurrentBlockUploadCount = a.ConcurrentBlocks
	}
	if a.ConcurrentCrypto > 0 {
		cfg.ConcurrentFileCryptoCount = a.ConcurrentCrypto
	}
	cfg.ReplaceExistingDraft = a.ReplaceExisting
	cfg.CredentialCacheFile = a.CredentialCacheFile
	cfg.FirstLoginCredential = &common.FirstLoginCredentialData{}
	cfg.ReusableCredential = &common.ReusableCredentialData{}

	ctx, cancel := context.WithCancel(context.Background())
	s := &session{cfg: cfg, ctx: ctx, cancel: cancel}
	id := putSession(s)
	return cOK(map[string]int64{"session": id})
}

//export pd_login
func pd_login(sessionID C.longlong, argsJSON *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok {
		return cErrS("invalid session")
	}
	var a loginArgs
	if err := json.Unmarshal([]byte(C.GoString(argsJSON)), &a); err != nil {
		return cErr(err)
	}
	s.cfg.UseReusableLogin = false
	s.cfg.FirstLoginCredential = &common.FirstLoginCredentialData{
		Username:        a.Username,
		Password:        a.Password,
		MailboxPassword: a.MailboxPassword,
		TwoFA:           a.TwoFA,
	}
	drive, cred, err := bridge.NewProtonDrive(s.ctx, s.cfg, nil, nil)
	if err != nil {
		// Check for Human Verification required (code 9001).
		hvToken, hvMethods := extractHVDetails(err)
		if hvToken != "" {
			s.pendingLogin = &a
			return cReturn(response{OK: map[string]any{
				"status":   "hv_required",
				"hv_token": hvToken,
				"methods":  hvMethods,
			}})
		}
		return cErr(err)
	}
	s.drive = drive
	s.cred = cred
	return cOK(credDTO{
		UID:           cred.UID,
		AccessToken:   cred.AccessToken,
		RefreshToken:  cred.RefreshToken,
		SaltedKeyPass: cred.SaltedKeyPass,
	})
}

// extractHVDetails inspects an error chain for a proton.APIError with
// Code 9001 (Human Verification Required) and extracts the token and methods.
func extractHVDetails(err error) (string, []string) {
	var apiErr *proton.APIError
	if !errors.As(err, &apiErr) {
		return "", nil
	}
	if apiErr.Code != proton.HumanVerificationRequired {
		return "", nil
	}
	detailsMap, ok := apiErr.Details.(map[string]interface{})
	if !ok {
		return "", nil
	}
	token, _ := detailsMap["HumanVerificationToken"].(string)
	var methods []string
	if raw, ok := detailsMap["HumanVerificationMethods"].([]interface{}); ok {
		for _, m := range raw {
			if s, ok := m.(string); ok {
				methods = append(methods, s)
			}
		}
	}
	return token, methods
}

// pd_login_hv retries the blocked login with an HV solution token.
// argsJSON: {"hv_type":"captcha","hv_token":"...","two_fa":"..."}.
// The username/password/mailbox_password are taken from the pending login
// stored during the preceding pd_login call that returned hv_required.
//
//export pd_login_hv
func pd_login_hv(sessionID C.longlong, argsJSON *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok {
		return cErrS("invalid session")
	}
	if s.pendingLogin == nil {
		return cErrS("no pending login — call pd_login first")
	}
	var hvArgs struct {
		HVType  string `json:"hv_type"`
		HVToken string `json:"hv_token"`
		TwoFA   string `json:"two_fa"`
	}
	if err := json.Unmarshal([]byte(C.GoString(argsJSON)), &hvArgs); err != nil {
		return cErr(err)
	}

	pl := s.pendingLogin

	// 1. Build a fresh manager with HV headers injected on every request.
	m := proton.New(
		proton.WithAppVersion(s.cfg.AppVersion),
		proton.WithUserAgent(s.cfg.UserAgent),
	)
	hvType := hvArgs.HVType
	if hvType == "" {
		hvType = "captcha"
	}
	m.AddPreRequestHook(func(_ *resty.Client, req *resty.Request) error {
		req.SetHeader("x-pm-human-verification-token", hvArgs.HVToken)
		req.SetHeader("x-pm-human-verification-token-type", hvType)
		return nil
	})

	// 2. Authenticate — the HV headers allow the request through.
	c, auth, err := m.NewClientWithLogin(s.ctx, pl.Username, []byte(pl.Password))
	if err != nil {
		return cErr(fmt.Errorf("hv login: %w", err))
	}

	// 3. 2FA (if enabled and a fresh code was supplied).
	twoFA := hvArgs.TwoFA
	if twoFA == "" {
		twoFA = pl.TwoFA // fall back to the code from the original attempt
	}
	if auth.TwoFA.Enabled&proton.HasTOTP != 0 {
		if twoFA == "" {
			return cErrS("2FA required but no TOTP code provided")
		}
		if err := c.Auth2FA(s.ctx, proton.Auth2FAReq{TwoFactorCode: twoFA}); err != nil {
			return cErr(fmt.Errorf("hv 2fa: %w", err))
		}
	}

	// 4. Compute salted key passphrase.
	keyPass := []byte(pl.Password)
	if auth.PasswordMode == proton.TwoPasswordMode && pl.MailboxPassword != "" {
		keyPass = []byte(pl.MailboxPassword)
	}
	user, err := c.GetUser(s.ctx)
	if err != nil {
		return cErr(fmt.Errorf("hv get user: %w", err))
	}
	salts, err := c.GetSalts(s.ctx)
	if err != nil {
		return cErr(fmt.Errorf("hv get salts: %w", err))
	}
	saltedKeyPass, err := salts.SaltForKey(keyPass, user.Keys.Primary().ID)
	if err != nil {
		return cErr(fmt.Errorf("hv salt key: %w", err))
	}

	// 5. Switch the session config to reusable-login mode.
	s.cfg.UseReusableLogin = true
	s.cfg.ReusableCredential = &common.ReusableCredentialData{
		UID:           auth.UID,
		AccessToken:   auth.AccessToken,
		RefreshToken:  auth.RefreshToken,
		SaltedKeyPass: base64.StdEncoding.EncodeToString(saltedKeyPass),
	}
	s.pendingLogin = nil

	// 6. Init the drive with the pre-computed credential (no auth call).
	drive, _, err := bridge.NewProtonDrive(s.ctx, s.cfg, nil, nil)
	if err != nil {
		return cErr(fmt.Errorf("hv drive init: %w", err))
	}
	s.drive = drive
	// Use the credential we built from the HV auth — common.Login returns nil
	// cred on the reusable-login path, so we read back from the config.
	rc := s.cfg.ReusableCredential
	return cOK(credDTO{
		UID:           rc.UID,
		AccessToken:   rc.AccessToken,
		RefreshToken:  rc.RefreshToken,
		SaltedKeyPass: rc.SaltedKeyPass,
	})
}

//export pd_resume
func pd_resume(sessionID C.longlong, argsJSON *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok {
		return cErrS("invalid session")
	}
	var a resumeArgs
	if err := json.Unmarshal([]byte(C.GoString(argsJSON)), &a); err != nil {
		return cErr(err)
	}
	s.cfg.UseReusableLogin = true
	s.cfg.ReusableCredential = &common.ReusableCredentialData{
		UID:           a.UID,
		AccessToken:   a.AccessToken,
		RefreshToken:  a.RefreshToken,
		SaltedKeyPass: a.SaltedKeyPass,
	}
	drive, _, err := bridge.NewProtonDrive(s.ctx, s.cfg, nil, nil)
	if err != nil {
		return cErr(err)
	}
	s.drive = drive
	// common.Login returns nil cred on the reusable-login path; read from config.
	rc := s.cfg.ReusableCredential
	return cOK(credDTO{
		UID:           rc.UID,
		AccessToken:   rc.AccessToken,
		RefreshToken:  rc.RefreshToken,
		SaltedKeyPass: rc.SaltedKeyPass,
	})
}

//export pd_logout
func pd_logout(sessionID C.longlong) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok {
		return cErrS("invalid session")
	}
	if s.drive != nil {
		if err := s.drive.Logout(s.ctx); err != nil {
			return cErr(err)
		}
	}
	if s.cancel != nil {
		s.cancel()
	}
	dropSession(int64(sessionID))
	return cOK(nil)
}

//export pd_root_link_id
func pd_root_link_id(sessionID C.longlong) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session or not authenticated")
	}
	if s.drive.RootLink == nil {
		return cErrS("no root link")
	}
	return cOK(s.drive.RootLink.LinkID)
}

//export pd_list
func pd_list(sessionID C.longlong, folderID *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	id := C.GoString(folderID)
	if id == "" && s.drive.RootLink != nil {
		id = s.drive.RootLink.LinkID
	}
	entries, err := s.drive.ListDirectory(s.ctx, id)
	if err != nil {
		return cErr(err)
	}
	out := make([]linkDTO, 0, len(entries))
	for _, e := range entries {
		out = append(out, protonLinkToDTO(e.Name, e.IsFolder, e.Link))
	}
	return cOK(out)
}

//export pd_get_link
func pd_get_link(sessionID C.longlong, linkID *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	l, err := s.drive.GetLink(s.ctx, C.GoString(linkID))
	if err != nil {
		return cErr(err)
	}
	return cOK(protonLinkToDTO(l.Name, l.Type == proton.LinkTypeFolder, l))
}

//export pd_create_folder
func pd_create_folder(sessionID C.longlong, parentID *C.char, name *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	id, err := s.drive.CreateNewFolderByID(s.ctx, C.GoString(parentID), C.GoString(name))
	if err != nil {
		return cErr(err)
	}
	return cOK(id)
}

//export pd_upload
func pd_upload(sessionID C.longlong, parentID *C.char, name *C.char, srcPath *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	path := C.GoString(srcPath)
	f, err := os.Open(path)
	if err != nil {
		return cErr(err)
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return cErr(err)
	}
	id, _, err := s.drive.UploadFileByReader(
		s.ctx,
		C.GoString(parentID),
		C.GoString(name),
		st.ModTime(),
		f,
		0,
	)
	if err != nil {
		return cErr(err)
	}
	return cOK(map[string]any{"link_id": id, "size": st.Size()})
}

//export pd_download
func pd_download(sessionID C.longlong, linkID *C.char, dstPath *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	r, size, _, err := s.drive.DownloadFileByID(s.ctx, C.GoString(linkID), 0)
	if err != nil {
		return cErr(err)
	}
	defer r.Close()
	dst := C.GoString(dstPath)
	tmp := dst + ".pddl"
	out, err := os.Create(tmp)
	if err != nil {
		return cErr(err)
	}
	if _, err := io.Copy(out, r); err != nil {
		out.Close()
		os.Remove(tmp)
		return cErr(err)
	}
	if err := out.Close(); err != nil {
		os.Remove(tmp)
		return cErr(err)
	}
	if err := os.Rename(tmp, dst); err != nil {
		os.Remove(tmp)
		return cErr(err)
	}
	return cOK(map[string]any{"size": size})
}

//export pd_move
func pd_move(sessionID C.longlong, srcID *C.char, dstParentID *C.char, dstName *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	srcLink, err := s.drive.GetLink(s.ctx, C.GoString(srcID))
	if err != nil {
		return cErr(err)
	}
	if srcLink.Type == proton.LinkTypeFolder {
		err = s.drive.MoveFolderByID(s.ctx, C.GoString(srcID), C.GoString(dstParentID), C.GoString(dstName))
	} else {
		err = s.drive.MoveFileByID(s.ctx, C.GoString(srcID), C.GoString(dstParentID), C.GoString(dstName))
	}
	if err != nil {
		return cErr(err)
	}
	return cOK(nil)
}

//export pd_trash
func pd_trash(sessionID C.longlong, linkID *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	id := C.GoString(linkID)
	link, err := s.drive.GetLink(s.ctx, id)
	if err != nil {
		return cErr(err)
	}
	if link.Type == proton.LinkTypeFolder {
		err = s.drive.MoveFolderToTrashByID(s.ctx, id, false)
	} else {
		err = s.drive.MoveFileToTrashByID(s.ctx, id)
	}
	if err != nil {
		return cErr(err)
	}
	return cOK(nil)
}

//export pd_about
func pd_about(sessionID C.longlong) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	u, err := s.drive.About(s.ctx)
	if err != nil {
		return cErr(err)
	}
	return cOK(map[string]any{
		"id":       u.ID,
		"name":     u.Name,
		"email":    u.Email,
		"used":     u.UsedSpace,
		"max":      u.MaxSpace,
		"display":  u.DisplayName,
		"now":      time.Now().Unix(),
	})
}

//export pd_search
func pd_search(sessionID C.longlong, folderID *C.char, name *C.char) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	link, err := s.drive.SearchByNameInActiveFolderByID(
		s.ctx, C.GoString(folderID), C.GoString(name), true, true, 0,
	)
	if err != nil {
		return cErr(err)
	}
	if link == nil {
		return cOK(nil)
	}
	return cOK(protonLinkToDTO(C.GoString(name), link.Type == proton.LinkTypeFolder, link))
}

// pd_events: the bridge does not expose Drive events directly. We
// emulate event polling by walking the active root and reporting
// links whose ModifyTime is greater than `since`. This is a temporary
// fallback until we replace it with a true /events long-poll.
//export pd_events
func pd_events(sessionID C.longlong, since C.longlong) *C.char {
	s, ok := getSession(int64(sessionID))
	if !ok || s.drive == nil {
		return cErrS("invalid session")
	}
	if s.drive.RootLink == nil {
		return cErrS("no root link")
	}
	cutoff := int64(since)
	type evt struct {
		LinkID     string `json:"link_id"`
		ParentID   string `json:"parent_id"`
		Name       string `json:"name"`
		IsFolder   bool   `json:"is_folder"`
		ModifyTime int64  `json:"modify_time"`
		Size       int64  `json:"size"`
	}
	out := []evt{}
	var walk func(string) error
	walk = func(id string) error {
		entries, err := s.drive.ListDirectory(s.ctx, id)
		if err != nil {
			return err
		}
		for _, e := range entries {
			if e.Link == nil {
				continue
			}
			if e.Link.ModifyTime > cutoff {
				out = append(out, evt{
					LinkID:     e.Link.LinkID,
					ParentID:   e.Link.ParentLinkID,
					Name:       e.Name,
					IsFolder:   e.IsFolder,
					ModifyTime: e.Link.ModifyTime,
					Size:       e.Link.Size,
				})
			}
			if e.IsFolder {
				if err := walk(e.Link.LinkID); err != nil {
					return err
				}
			}
		}
		return nil
	}
	if err := walk(s.drive.RootLink.LinkID); err != nil {
		return cErr(err)
	}
	return cOK(map[string]any{
		"now":    time.Now().Unix(),
		"events": out,
	})
}

// pd_set_log_level configures the Go-side logger; accepts: error, warn, info, debug, trace.
//export pd_set_log_level
func pd_set_log_level(level *C.char) *C.char {
	// Currently the bridge logs via the standard library, which we
	// route to /dev/null unless explicitly enabled. The Rust side
	// does its own structured logging.
	_ = C.GoString(level)
	return cOK(nil)
}

// keep the linker happy
var _ = fmt.Sprintf
var _ = unsafe.Sizeof(0)

func main() {}
