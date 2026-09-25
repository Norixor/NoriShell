//go:build !windows

package main

import "os"

func replaceAndSync(source, destination, directory string) (bool, error) {
	if err := os.Rename(source, destination); err != nil {
		return false, err
	}
	dir, err := os.Open(directory)
	if err != nil {
		return true, err
	}
	defer dir.Close()
	return true, dir.Sync()
}
