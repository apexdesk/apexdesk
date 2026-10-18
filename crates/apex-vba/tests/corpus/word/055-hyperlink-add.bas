' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: https://example.com/
Sub Main()
    Dim r As Range
    Set r = ActiveDocument.Paragraphs(3).Range
    r.MoveEnd wdCharacter, -1
    ActiveDocument.Hyperlinks.Add Anchor:=r, Address:="https://example.com/", TextToDisplay:="The end."
    Debug.Print ActiveDocument.Hyperlinks.Count
    Debug.Print ActiveDocument.Hyperlinks(1).Address
End Sub
