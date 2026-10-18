' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Mary Jane
' corpus-prints: John Smith
Sub Main()
    Range("A1").Value = "mary jane"
    Range("A2").Value = "JOHN SMITH"
    Dim c As Range
    For Each c In Range("A1:A2")
        c.Value = StrConv(c.Value, vbProperCase)
    Next c
    Debug.Print Range("A1").Value
    Debug.Print Range("A2").Value
End Sub
