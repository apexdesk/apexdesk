' corpus-source: generated
' corpus-expect: ok
' corpus-note: Worksheet.Sort (the SortFields/SetRange/Apply object) is not supported; Range.Sort works
' corpus-flagged: no
' corpus-prints: a
' corpus-prints: c
Sub Main()
    Range("A1").Value = "c": Range("A2").Value = "a": Range("A3").Value = "b"
    With ActiveSheet.Sort
        .SortFields.Clear
        .SortFields.Add Key:=Range("A1:A3"), Order:=xlAscending
        .SetRange Range("A1:A3")
        .Header = xlNo
        .Apply
    End With
    Debug.Print Range("A1").Value
    Debug.Print Range("A3").Value
End Sub
