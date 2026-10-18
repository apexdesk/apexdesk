' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: The quick brown fox.
' corpus-prints: Jumps over the lazy dog.
' corpus-prints: The end.
Sub Main()
    Dim p As Paragraph
    For Each p In ActiveDocument.Paragraphs
        Debug.Print Replace(p.Range.Text, vbCr, "")
    Next p
End Sub
