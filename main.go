// 1MbPDF: barra laterale che comprime immagini e PDF in un PDF da massimo 1 MB.
package main

import (
	"os"

	"onembpdf/internal/ui"
)

func main() {
	if err := ui.Run(os.Args[1:]); err != nil {
		ui.Fatal(err)
	}
}
