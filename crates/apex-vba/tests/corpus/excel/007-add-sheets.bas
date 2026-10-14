' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
' corpus-prints: Jan
' corpus-prints: Mar
Sub Main()
    Dim m As Variant, ws As Worksheet
    For Each m In Array("Jan", "Feb", "Mar")
        Set ws = Worksheets.Add(After:=Worksheets(Worksheets.Count))
        ws.Name = m
    Next m
    Debug.Print Worksheets.Count
    Debug.Print Worksheets(2).Name
    Debug.Print Worksheets(4).Name
End Sub
