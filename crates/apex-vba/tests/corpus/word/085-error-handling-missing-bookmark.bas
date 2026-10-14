' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: no bookmark
Sub Main()
    Dim r As Range
    On Error GoTo nope
    Set r = ActiveDocument.Bookmarks("Missing").Range
    Debug.Print "found"
    Exit Sub
nope:
    Debug.Print "no bookmark"
End Sub
