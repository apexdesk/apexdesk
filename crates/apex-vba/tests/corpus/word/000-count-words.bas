' corpus-kind: word
' corpus-source: generated
' corpus-prints: 3
' corpus-flagged: no
Sub Main()
    Dim p As Paragraph, n As Long
    For Each p In ActiveDocument.Paragraphs
        n = n + 1
    Next p
    Debug.Print n
End Sub
