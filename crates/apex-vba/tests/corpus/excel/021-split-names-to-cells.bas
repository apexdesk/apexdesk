' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Smith
' corpus-prints: John
Sub Main()
    Dim p() As String
    Range("A1").Value = "Smith, John"
    p = Split(Range("A1").Value, ", ")
    Range("B1").Value = p(0)
    Range("C1").Value = p(1)
    Debug.Print Range("B1").Value
    Debug.Print Range("C1").Value
End Sub
