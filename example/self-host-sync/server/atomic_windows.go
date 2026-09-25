//go:build windows

package main

import "golang.org/x/sys/windows"

func replaceAndSync(source, destination, _ string) (bool, error) {
	from, err := windows.UTF16PtrFromString(source)
	if err != nil {
		return false, err
	}
	to, err := windows.UTF16PtrFromString(destination)
	if err != nil {
		return false, err
	}
	// Windows does not offer POSIX directory fsync through os.File.Sync.
	// MoveFileEx with WRITE_THROUGH provides the platform's durable replace.
	if err := windows.MoveFileEx(from, to, windows.MOVEFILE_REPLACE_EXISTING|windows.MOVEFILE_WRITE_THROUGH); err != nil {
		return false, err
	}
	return true, nil
}
