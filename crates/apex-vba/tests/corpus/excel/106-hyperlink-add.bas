' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Example
' corpus-prints: https://example.com/
Sub Main()
    ActiveSheet.Hyperlinks.Add Anchor:=Range("A1"), Address:="https://example.com/", TextToDisplay:="Example"
    Debug.Print Range("A1").Value
    Debug.Print Range("A1").Hyperlinks(1).Address
End Sub
